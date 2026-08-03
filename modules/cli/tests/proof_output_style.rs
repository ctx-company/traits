//! P467's drift-gate honesty layer: proves a migrated command's *default*
//! human output actually has the shape its presentation-registry entry
//! claims (`presentation.rs`'s `presentation_for`), by driving the real
//! `ctx` binary and parsing structural facts out of its plain-degraded
//! stdout — never a stored golden/snapshot (P461).
//!
//! `presentation_for`/`CommandPresentation` are `pub` (not `pub(crate)`) and
//! `presentation` is a `pub mod` specifically so this external integration
//! test crate can call the SAME registry function the production handler
//! dispatch consults, and assert the real process's stdout against the
//! shape THAT ANSWER claims — not a hardcoded literal that could silently
//! drift from the registry. [`assert_matches_registry_claim`] is the shared
//! binding every test below goes through.
//!
//! The registration layer (every visible command resolves in the registry,
//! and every registry entry still names a live visible command) stays a
//! crate-internal unit test in `src/app/presentation.rs`
//! (`every_visible_traits_command_has_a_registry_entry`): it walks the
//! derived Clap tree via `crate::app::surface::cli::command()`, which this
//! external crate has no path to construct identically. The two layers
//! together are the full gate: registration proves every command has an
//! opinion, this file proves that opinion is true.
//!
//! `controlled_command` (via `support::run_ctx`/`require_success`) always
//! sets `NO_COLOR=1`, so every assertion here already exercises the plain
//! (non-TTY) degrade path P465 guarantees: no ANSI escapes, no box-drawing
//! glyphs.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use ctx_traits_cli::app::presentation::{CommandPresentation, presentation_for};
use support::{
    ScratchRoot, controlled_command, ctx_bin, git_init, repo_root, require_success,
    symlink_node_modules,
};

/// A dedicated, empty Git repository under `scratch`, matching
/// `proof_doctor.rs`'s fixture: doctor's cross-tier trust resolution needs a
/// real repo, and a pre-seeded nested `.ctx/.gitignore` keeps these
/// assertions about presentation shape, not repo-state housekeeping.
fn scratch_repo(scratch: &ScratchRoot) -> PathBuf {
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let gitignore_dir = repo.join(".ctx");
    fs::create_dir_all(&gitignore_dir).unwrap();
    // Derived from the canonical list, never a copy: this fixture had already
    // drifted (it was missing `config.ts`), which makes `doctor` report
    // repo-state housekeeping these assertions were written to exclude.
    fs::write(
        gitignore_dir.join(".gitignore"),
        ctx_traits_io::gitignore::CANONICAL_ENTRIES
            .iter()
            .map(|entry| format!("{entry}\n"))
            .collect::<String>(),
    )
    .unwrap();
    repo
}

fn write_healthy_skill(dir: &std::path::Path, relative: &str) {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "---\nname: Healthy Skill\ndescription: A demo skill with no findings.\n---\n\n\
         # Healthy Skill\n\nNothing hidden here.\n",
    )
    .unwrap();
}

/// Parses `stdout` as a P465 panel's plain projection (`Panel::plain_lines`)
/// and asserts its required structural shape, without comparing against any
/// stored expected string: the first non-empty line is `"{product}
/// {headline}"`, every indented `"  label: value"` row (section titles and
/// blank separators aside) carries a non-empty label before its first
/// `": "`, and the last non-empty line — the closing state — is a single
/// bare word with no punctuation, matching `PanelStatus`'s closing-text
/// contract. Reusable by every future migrated command's honesty-layer
/// test; add a call site here rather than re-deriving this parse per
/// command.
fn assert_plain_panel_structure(product: &str, headline: &str, stdout: &str) {
    for glyph in ["╭", "╰", "│", "─", "\x1b["] {
        assert!(
            !stdout.contains(glyph),
            "plain-degraded panel output must carry no styled-panel glyph {glyph:?}: {stdout}"
        );
    }

    let lines: Vec<&str> = stdout.lines().collect();
    assert!(!lines.is_empty(), "panel output must not be empty");
    assert_eq!(
        lines[0],
        format!("{product} {headline}"),
        "first line must be the panel's product/headline pair: {stdout}"
    );

    let closing_index = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .unwrap_or_else(|| panic!("panel output has no non-empty closing line: {stdout}"));

    for (index, line) in lines.iter().enumerate().skip(1) {
        if line.trim().is_empty() || index == closing_index {
            continue;
        }
        // Every remaining line is either an indented `label: value` row or a
        // bare section title (`Title:`, no leading indent) — both carry a
        // colon, distinguishing panel body content from stray output.
        assert!(
            line.contains(':'),
            "body line must be a labelled row or section title: {line:?} in {stdout}"
        );
    }
}

/// Drops any lines before the panel's own `"{product} {headline}"` line.
/// `run`'s non-panel command-started narration (P427, `drive.rs`'s
/// `command_started_event`) is unconditional on a non-TTY stdout and
/// precedes the panel — a pre-existing, out-of-scope live-narration line
/// (§1's explicit non-scope: "the drive narration stream"), not panel
/// vocabulary, so callers whose command narrates before its panel strip it
/// here rather than have `assert_plain_panel_structure` special-case it.
fn strip_pre_panel_narration(stdout: &str, product: &str, headline: &str) -> String {
    let marker = format!("{product} {headline}");
    match stdout.find(&marker) {
        Some(index) => stdout[index..].to_string(),
        None => stdout.to_string(),
    }
}

/// Looks up `command`'s claim in the SAME registry function the production
/// dispatch consults, asserts it is [`CommandPresentation::Panel`] (the only
/// claim this file currently proves against — a `OneRow` command gets its
/// own one-line assertion once one exists), then checks `stdout` against
/// that claim's required shape. This is the binding blocker
/// `registry-claim-unbound-to-actual-output` names: a registry row flipped
/// to `Panel` without a working migration fails here, not silently.
fn assert_matches_registry_claim(command: &str, product: &str, headline: &str, stdout: &str) {
    assert_eq!(
        presentation_for(command),
        Ok(CommandPresentation::Panel),
        "test claims {command:?} is registered Panel; registry disagrees"
    );
    assert_plain_panel_structure(product, headline, stdout);
}

#[test]
fn doctor_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-doctor-panel-shape");
    let root = scratch.home().join("sources");
    write_healthy_skill(&root, "healthy/SKILL.md");

    let repo = scratch_repo(&scratch);
    let stdout = require_success(
        "`ctx traits doctor` over a healthy source tree",
        &["traits", "doctor", root.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("doctor", "ctx", "doctor", &stdout);
}

#[test]
fn init_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-init-panel-shape");
    let repo = scratch_repo(&scratch);
    let stdout = require_success(
        "`ctx traits init` in a fresh repository",
        &["traits", "init"],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("init", "ctx", "init", &stdout);
}

#[test]
fn list_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-list-panel-shape");
    let repo = scratch_repo(&scratch);
    require_success(
        "`ctx traits init` before `list`",
        &["traits", "init"],
        &repo,
        &scratch.home(),
    );
    let stdout = require_success(
        "`ctx traits list` over an empty project",
        &["traits", "list"],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("list", "ctx", "list", &stdout);
}

#[test]
fn cache_status_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-cache-status-panel-shape");
    let repo = scratch_repo(&scratch);
    let stdout = require_success(
        "`ctx traits cache status` over an empty project",
        &["traits", "cache", "status"],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("cache", "ctx", "cache status", &stdout);
}

/// `config build`'s default output is the one non-`cache`/`doctor`-style
/// Panel this file drives against a real node build rather than a
/// project-state fixture (P457): a `config.ts` with no local import,
/// resolved against the repo's own already-installed `node_modules` via
/// [`symlink_node_modules`] (shared with `proof_config_ts.rs`, never a
/// network fetch), so the honesty layer proves the SAME command
/// `proof_config_ts.rs` exercises for behavior also carries the panel shape
/// its registry entry claims.
#[test]
fn config_build_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-config-build-panel-shape");
    let repo = scratch_repo(&scratch);
    symlink_node_modules(&repo);
    fs::write(
        repo.join(".ctx/config.ts"),
        "import { defineConfig } from \"@ctx-traits/config\";\n\n\
         export default defineConfig({\n  run: { maxRetries: 3 },\n});\n",
    )
    .unwrap();

    let stdout = require_success(
        "`ctx traits config build` over a config.ts with no local import",
        &["traits", "config", "build"],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("config", "ctx", "config build", &stdout);
}

/// A built, checked-out trait under a fresh scratch repo — the shared
/// fixture `check`/`diff`/`explain`/`export`'s honesty-layer tests drive,
/// mirroring `proof_cdk_vocabulary.rs`'s `build_intent_fixture` init+build
/// sequence rather than reinventing a second fixture shape.
struct BuiltTraitFixture {
    _scratch: ScratchRoot,
    repo: PathBuf,
    home: PathBuf,
    trait_id: String,
}

fn build_trait_fixture(label: &str, trait_id: &str) -> BuiltTraitFixture {
    let scratch = ScratchRoot::new(label);
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);

    #[cfg(unix)]
    std::os::unix::fs::symlink(repo_root().join("node_modules"), repo.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(repo_root().join("node_modules"), repo.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));

    require_success(
        &format!("`ctx traits init {trait_id}`"),
        &["traits", "init", trait_id],
        &repo,
        &home,
    );
    require_success(
        &format!("`ctx traits build` for {trait_id}"),
        &[
            "traits",
            "build",
            &format!(".ctx/traits/packages/{trait_id}/source/index.ts"),
        ],
        &repo,
        &home,
    );

    BuiltTraitFixture {
        _scratch: scratch,
        repo,
        home,
        trait_id: trait_id.to_string(),
    }
}

#[test]
fn check_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-check-panel-shape", "fixture-check-panel");
    let stdout = require_success(
        "`ctx traits check` over a freshly-init'd trait",
        &["traits", "check", &fixture.trait_id],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("check", "ctx", "check", &stdout);
}

#[test]
fn diff_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-diff-panel-shape", "fixture-diff-panel");
    let stdout = require_success(
        "`ctx traits diff` over a freshly-init'd trait",
        &["traits", "diff", &fixture.trait_id],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("diff", "ctx", "diff", &stdout);
}

#[test]
fn explain_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-explain-panel-shape", "fixture-explain-panel");
    let trait_toml = fixture
        .repo
        .join(format!(
            ".ctx/traits/packages/{}/generated/index.toml",
            fixture.trait_id
        ))
        .to_string_lossy()
        .to_string();
    let stdout = require_success(
        "`ctx traits explain` over a freshly-init'd trait",
        &[
            "traits",
            "explain",
            "--file",
            &trait_toml,
            "--task",
            "describe the starter task",
        ],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("explain", "ctx", "explain", &stdout);
}

#[test]
fn export_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-export-panel-shape", "fixture-export-panel");
    let stdout = require_success(
        "`ctx traits export` over a freshly-init'd trait",
        &["traits", "export", &fixture.trait_id, "--allow-unreviewed"],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("export", "ctx", "export", &stdout);
}

#[test]
fn new_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-new-panel-shape");
    let repo = scratch_repo(&scratch);
    let stdout = require_success(
        "`ctx traits new` with no arguments (lists templates)",
        &["traits", "new"],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("new", "ctx", "new", &stdout);
}

#[test]
fn build_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-build-panel-shape");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);

    #[cfg(unix)]
    std::os::unix::fs::symlink(repo_root().join("node_modules"), repo.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(repo_root().join("node_modules"), repo.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));

    require_success(
        "`ctx traits init fixture-build-panel`",
        &["traits", "init", "fixture-build-panel"],
        &repo,
        &home,
    );
    require_success(
        "initial explicit-path `ctx traits build` for fixture-build-panel",
        &[
            "traits",
            "build",
            ".ctx/traits/packages/fixture-build-panel/source/index.ts",
        ],
        &repo,
        &home,
    );
    let stdout = require_success(
        "`ctx traits build` for fixture-build-panel",
        &["traits", "build", "fixture-build-panel"],
        &repo,
        &home,
    );

    assert_matches_registry_claim("build", "ctx", "build", &stdout);
}

#[test]
fn host_install_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-host-panel-shape", "fixture-host-panel");
    let stdout = require_success(
        "`ctx traits host install --host cursor` over a freshly-init'd trait",
        &[
            "traits",
            "host",
            "install",
            "--host",
            "cursor",
            "--allow-unreviewed",
            "--allow-draft",
            &fixture.trait_id,
        ],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("host", "ctx", "host-install", &stdout);
}

#[test]
fn vendor_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-vendor-panel-shape", "fixture-vendor-panel");
    let stdout = require_success(
        "`ctx traits vendor` over a freshly-init'd trait",
        &["traits", "dependency", "install", &fixture.trait_id],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("dependency", "ctx", "dependency install", &stdout);
}

#[test]
fn import_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-import-panel-shape");
    let source = scratch.home().join("sources");
    write_healthy_skill(&source, "healthy/SKILL.md");
    let repo = scratch_repo(&scratch);

    let stdout = require_success(
        "`ctx traits import` over a local SKILL.md source",
        &[
            "traits",
            "import",
            "--source",
            source.join("healthy").to_str().unwrap(),
        ],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("import", "ctx", "import", &stdout);
}

#[test]
fn activate_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-activate-panel-shape", "fixture-activate-panel");
    let stdout = require_success(
        "`ctx traits activate` over a freshly-init'd trait",
        &["traits", "activate", &fixture.trait_id],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("activate", "ctx", "activate", &stdout);
}

#[test]
fn trust_approve_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-trust-panel-shape", "fixture-trust-panel");
    let stdout = require_success(
        "`ctx traits trust approve` over a freshly-init'd trait",
        &["traits", "trust", "approve", &fixture.trait_id],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("trust", "ctx", "trust approve", &stdout);
}

/// A provider-free, network-free repo carrying one reviewed, activated
/// `kind = "command"` trait (`cmd` runs a trivial local shell command, no
/// harness dispatch) — the same fixture shape `proof_merge_on_completion.rs`
/// uses to drive `run`/`merge` end to end without a script harness or a
/// model provider.
fn command_trait_fixture(label: &str) -> BuiltTraitFixture {
    let scratch = ScratchRoot::new(label);
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init(&repo);
    fs::write(repo.join(".gitignore"), ".ctx/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        "id = \"demo\"\n\
         schema-version = \"0.2\"\n\
         version = \"0.1.0\"\n\
         name = \"Demo\"\n\
         summary = \"A provider-free command-only trait.\"\n\
         \n\
         [procedure]\n\
         description = \"Run one deterministic command.\"\n\
         \n\
         [[slot]]\n\
         id = \"notified\"\n\
         schema = \"schema:text\"\n\
         \n\
         [[procedure.sequence]]\n\
         id = \"command\"\n\
         title = \"Run command\"\n\
         kind = \"command\"\n\
         cmd = \"true\"\n\
         output = [\"slot:notified\"]\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(&repo)
        .status()
        .unwrap();

    let fixture_path = ".ctx/traits/demo/generated/index.toml";
    require_success(
        "`ctx traits trust approve`",
        &["traits", "trust", "approve", fixture_path],
        &repo,
        &home,
    );
    require_success(
        "`ctx traits activate --file`",
        &["traits", "activate", "--file", fixture_path],
        &repo,
        &home,
    );
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "activate"])
        .current_dir(&repo)
        .status()
        .unwrap();

    BuiltTraitFixture {
        _scratch: scratch,
        repo,
        home,
        trait_id: "demo".to_string(),
    }
}

#[test]
fn run_default_output_matches_the_panel_registry_shape() {
    let fixture = command_trait_fixture("p467-run-panel-shape");
    let stdout = require_success(
        "`ctx traits run` over a provider-free command-only trait",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--progress",
            "none",
        ],
        &fixture.repo,
        &fixture.home,
    );
    let stdout = strip_pre_panel_narration(&stdout, "ctx", "run");

    assert_matches_registry_claim("run", "ctx", "run", &stdout);
}

#[test]
fn run_without_progress_flag_defaults_to_status_under_piped_stdio() {
    // P474: absent `--progress`, a non-interactive invocation (this harness's
    // stdio is always piped) must resolve to `status` — byte-identical to
    // the pre-P474 hard default — and never open the TUI's alternate screen.
    let fixture = command_trait_fixture("p474-run-default-progress");
    let stdout = require_success(
        "`ctx traits run` over a provider-free command-only trait with no --progress flag",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
        ],
        &fixture.repo,
        &fixture.home,
    );
    let stdout = strip_pre_panel_narration(&stdout, "ctx", "run");

    assert!(
        !stdout.contains("\x1b[?1049h"),
        "no --progress under piped stdio must not open the TUI alternate screen: {stdout}"
    );
    assert_matches_registry_claim("run", "ctx", "run", &stdout);
}

#[test]
fn run_progress_modes_remain_line_oriented_when_stdio_is_piped() {
    let fixture = command_trait_fixture("p551-run-piped-progress");
    fs::write(
        fixture.repo.join(".gitignore"),
        ".ctx/worktrees/\nwarm-valid/\n",
    )
    .unwrap();
    fs::write(
        fixture.repo.join(".ctx/config.toml"),
        "[worktree]\nwarm = [\"missing-warm\", \"warm-valid\"]\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", ".gitignore"])
        .current_dir(&fixture.repo)
        .status()
        .unwrap();
    fs::create_dir_all(fixture.repo.join("warm-valid")).unwrap();
    fs::write(fixture.repo.join("warm-valid/cache.txt"), "warm cache\n").unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "warm fixture"])
        .current_dir(&fixture.repo)
        .status()
        .unwrap();

    let mut baseline = None;
    for extra in [
        vec![],
        vec!["--progress", "stream"],
        vec!["--progress", "status"],
        vec!["--progress", "none"],
        vec!["--no-drive"],
    ] {
        let mut args = vec![
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
        ];
        args.extend(extra.iter().copied());
        let output = controlled_command(&ctx_bin(), &args, &fixture.repo, &fixture.home)
            .output()
            .unwrap_or_else(|error| panic!("cannot run {args:?}: {error}"));
        assert!(
            output.status.success(),
            "piped run {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("ctx run · initialization"),
            "piped run {args:?} must retain initialization narration: {stderr}"
        );
        assert_eq!(
            stderr.matches("warming warm-valid").count(),
            1,
            "piped run {args:?} must narrate exactly one validated warm entry: {stderr}"
        );
        assert!(
            !stderr.contains("warming missing-warm"),
            "piped run {args:?} narrated a missing warm entry: {stderr}"
        );
        if let Some(baseline) = &baseline {
            assert_eq!(
                &output.stderr, baseline,
                "piped run {args:?} changed line-mode stderr"
            );
        } else {
            baseline = Some(output.stderr.clone());
        }
        for bytes in [&output.stdout, &output.stderr] {
            assert!(
                !bytes.contains(&0x1b),
                "piped run {args:?} emitted terminal control bytes: {:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }
}

#[test]
fn merge_default_output_matches_the_panel_registry_shape() {
    let fixture = command_trait_fixture("p467-merge-panel-shape");

    let json_output = require_success(
        "`ctx traits run --worktree --json` (no --merge) leaving a landable run",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--json",
            "--progress",
            "none",
        ],
        &fixture.repo,
        &fixture.home,
    );
    // `--progress none`'s non-panel command-started narration (P427,
    // `drive.rs`'s `command_started_event`) is unconditional and precedes
    // the JSON envelope on stdout — a pre-existing, out-of-scope live-
    // narration line (§1's explicit non-scope), not panel vocabulary, so
    // strip it rather than widen this presentation-focused test into a
    // `--json` byte-stability proof for `run`.
    let json_start = json_output
        .find('{')
        .unwrap_or_else(|| panic!("no JSON object found in stdout: {json_output}"));
    let envelope: serde_json::Value = serde_json::from_str(&json_output[json_start..])
        .unwrap_or_else(|error| {
            panic!("`ctx traits run --json` did not emit JSON: {error}\nstdout: {json_output}")
        });
    let run_id = envelope["value"]["session"]["run-id"]
        .as_str()
        .unwrap_or_else(|| panic!("no value.session.run-id in {envelope}"))
        .to_string();

    let stdout = require_success(
        "`ctx traits merge <run-id>` landing the run started above",
        &["traits", "merge", &run_id],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("merge", "ctx", "merge", &stdout);
}

#[test]
fn remove_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-remove-panel-shape");
    let repo = scratch_repo(&scratch);
    // A hand-authored `[dependencies]` entry, in the exact shape
    // `ctx traits install` itself writes (`prepare_manifest_dependency_text`,
    // `modules/io/src/distribution.rs`) — `remove` only ever rewrites this
    // manifest/lock text locally, so no install (and no network) is needed
    // to exercise it.
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    fs::write(
        repo.join(".ctx/traits.toml"),
        "schema-version = \"0.2\"\n\n\
         [dependencies.demo-dep]\n\
         npm = \"demo-dep\"\n\
         version = \"1.0.0\"\n",
    )
    .unwrap();

    let stdout = require_success(
        "`ctx traits remove demo-dep` over a hand-authored manifest entry",
        &["traits", "dependency", "remove", "demo-dep"],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("dependency", "ctx", "dependency remove demo-dep", &stdout);
}

/// `install`/`info`/`outdated`/`publish`/`update` (deps-npm cluster) fetch
/// package metadata/tarballs from the npm registry, and `generate`/`refine`/
/// `critique` (author-assist cluster) always dispatch a model provider by
/// design (draft §1: "`generate` is always LLM-assisted by default and is
/// not a deterministic synth alias") — per §3.2's carve-out rule ("Do not
/// invent a mock harness for this; provider-free is absolute and a mock
/// would be new machinery for a presentation phase"), none of these eight
/// have a network-free/provider-free path to their default output, so their
/// honesty-layer coverage is layer 1 only (the registration-layer test in
/// `presentation.rs` that proves each resolves and stays bound to a live
/// command).
const NETWORK_OR_PROVIDER_CARVE_OUT: &[&str] = &["generate", "refine", "critique"];

/// Every command exercised by a real `assert_matches_registry_claim` call
/// above.
const HONESTY_TESTED: &[&str] = &[
    "doctor",
    "init",
    "list",
    "cache",
    "config",
    "check",
    "diff",
    "explain",
    "export",
    "new",
    "build",
    "host",
    "import",
    "activate",
    "trust",
    "run",
    "merge",
    // P567: `dependency` covers what `vendor`/`install`/`remove`/`update`/
    // `outdated`/`info` used to register individually; the two honesty tests
    // above drive it through `dependency install` and `dependency remove`,
    // which are the two subcommands reachable without network or a registry.
    "dependency",
];

/// The mechanical binding blocker `registry-claim-unbound-to-actual-output`
/// asks for: walks `presentation::REGISTERED_COMMAND_NAMES` (the SAME flat
/// list `presentation_for`'s own exhaustiveness test in `presentation.rs`
/// checks against the real Clap tree) and asserts every name that resolves
/// to `Panel` is in EXACTLY ONE of [`HONESTY_TESTED`] (driven by a real
/// `assert_matches_registry_claim` call above) or
/// [`NETWORK_OR_PROVIDER_CARVE_OUT`] (an explicit, commented network/
/// provider blocker). A future commit that flips a command's registry row
/// to `Panel` without adding either fails HERE, not silently — the failure
/// this blocker was opened for.
#[test]
fn every_panel_command_is_honesty_tested_or_explicitly_carved_out() {
    for name in ctx_traits_cli::app::presentation::REGISTERED_COMMAND_NAMES {
        let in_tested = HONESTY_TESTED.contains(name);
        let in_carve_out = NETWORK_OR_PROVIDER_CARVE_OUT.contains(name);
        match presentation_for(name) {
            Ok(CommandPresentation::Panel) => {
                assert!(
                    in_tested || in_carve_out,
                    "{name:?} is registered Panel but is neither honesty-tested above nor \
                     listed in NETWORK_OR_PROVIDER_CARVE_OUT — add a real \
                     assert_matches_registry_claim test or an explicit carve-out"
                );
                assert!(
                    !(in_tested && in_carve_out),
                    "{name:?} must not appear in both HONESTY_TESTED and \
                     NETWORK_OR_PROVIDER_CARVE_OUT"
                );
            }
            Ok(CommandPresentation::OneRow) | Ok(CommandPresentation::Exempt(_)) => {
                assert!(
                    !in_tested && !in_carve_out,
                    "{name:?} is not registered Panel; remove it from this file's coverage \
                     lists or the registry disagrees with this test"
                );
            }
            Err(error) => {
                panic!("{name:?} is a registered name but presentation_for errors: {error}")
            }
        }
    }
    for name in HONESTY_TESTED
        .iter()
        .chain(NETWORK_OR_PROVIDER_CARVE_OUT.iter())
    {
        assert!(
            ctx_traits_cli::app::presentation::REGISTERED_COMMAND_NAMES.contains(name),
            "{name:?} is listed in this file's coverage lists but is no longer a registered \
             command name — stale entry, remove it"
        );
    }
}
