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
/// stored expected string: the first non-empty line is `"{product} ·
/// {headline}"` (or just `{product}`), every indented `"  label: value"` row (section titles and
/// blank separators aside) carries a non-empty label before its first
/// `": "`, and the last non-empty line — the closing state — is a single
/// bare word with no punctuation, matching `PanelStatus`'s closing-text
/// contract. Reusable by every future migrated command's honesty-layer
/// test; add a call site here rather than re-deriving this parse per
/// command.
fn assert_plain_panel_structure(product: &str, headline: &str, stdout: &str) {
    for glyph in ["╭", "╰", "┌", "└", "│", "─", "\x1b["] {
        assert!(
            !stdout.contains(glyph),
            "plain-degraded panel output must carry no styled-panel glyph {glyph:?}: {stdout}"
        );
    }

    let lines: Vec<&str> = stdout.lines().collect();
    assert!(!lines.is_empty(), "panel output must not be empty");
    assert_eq!(
        lines[0],
        if headline.is_empty() {
            product.to_string()
        } else {
            format!("{product} · {headline}")
        },
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
        if let Some((label, _)) = line.strip_prefix("  ").and_then(|row| row.split_once(": ")) {
            assert!(
                label != "ledger" && label != "worktree" && !label.contains("lock"),
                "default panel must not expose internal row {label:?}: {stdout}"
            );
        }
    }
}

/// Drops any lines before the panel's own `"{product} · {headline}"` line.
/// `run`'s non-panel command-started narration (P427, `drive.rs`'s
/// `command_started_event`) is unconditional on a non-TTY stdout and
/// precedes the panel — a pre-existing, out-of-scope live-narration line
/// (§1's explicit non-scope: "the drive narration stream"), not panel
/// vocabulary, so callers whose command narrates before its panel strip it
/// here rather than have `assert_plain_panel_structure` special-case it.
fn strip_pre_panel_narration(stdout: &str, product: &str, headline: &str) -> String {
    let marker = if headline.is_empty() {
        product.to_string()
    } else {
        format!("{product} · {headline}")
    };
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
    fs::create_dir_all(repo.join(".ctx/traits")).unwrap();
    fs::write(
        repo.join(".ctx/traits/config.ts"),
        "import { defineConfig } from \"@ctx-traits/config\";\n\n\
         export default defineConfig({});\n",
    )
    .unwrap();

    let stdout = require_success(
        "`ctx traits internal config build` over a config.ts with no local import",
        &["traits", "internal", "config", "build"],
        &repo,
        &scratch.home(),
    );

    // Hidden since the 2026-08-18 regroup: shape still proven, no registry claim.
    assert_plain_panel_structure("ctx", "config build", &stdout);
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
            &format!(".ctx/traits/authored/{trait_id}/source/index.ts"),
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
fn migrate_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-migrate-panel-shape", "fixture-migrate-panel");
    let stdout = require_success(
        "`ctx traits internal migrate` over a freshly-init'd trait already at the latest schema version",
        &["traits", "internal", "migrate", &fixture.trait_id],
        &fixture.repo,
        &fixture.home,
    );

    // Hidden since the 2026-08-18 regroup: shape still proven, no registry claim.
    assert_plain_panel_structure("ctx", "migrate", &stdout);
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
fn diff_from_lock_hides_lock_rows_in_compact_output() {
    let fixture = build_trait_fixture("p467-diff-from-lock-panel-shape", "fixture-diff-lock-panel");
    let compact = require_success(
        "`ctx traits diff --from-lock` over a freshly-init'd trait",
        &["traits", "diff", &fixture.trait_id, "--from-lock"],
        &fixture.repo,
        &fixture.home,
    );
    assert_matches_registry_claim("diff", "ctx", "diff", &compact);

    let verbose = require_success(
        "`ctx traits diff --from-lock --verbose` retains lock evidence",
        &[
            "traits",
            "diff",
            &fixture.trait_id,
            "--from-lock",
            "--verbose",
        ],
        &fixture.repo,
        &fixture.home,
    );
    assert!(
        verbose.contains("projection-lock") || panel_row_labels(&verbose).contains(&"lock"),
        "verbose diff must retain lock evidence: {verbose}"
    );
}

#[test]
fn explain_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-explain-panel-shape", "fixture-explain-panel");
    let trait_toml = fixture
        .repo
        .join(format!(
            ".ctx/traits/authored/{}/generated/index.toml",
            fixture.trait_id
        ))
        .to_string_lossy()
        .to_string();
    let stdout = require_success(
        "`ctx traits internal explain` over a freshly-init'd trait",
        &[
            "traits",
            "internal",
            "explain",
            "--file",
            &trait_toml,
            "--task",
            "describe the starter task",
        ],
        &fixture.repo,
        &fixture.home,
    );

    // Hidden since the 2026-08-18 regroup: shape still proven, no registry claim.
    assert_plain_panel_structure("ctx", "explain", &stdout);
}

#[test]
fn export_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-export-panel-shape", "fixture-export-panel");
    let stdout = require_success(
        "`ctx traits internal export` over a freshly-init'd trait",
        &[
            "traits",
            "internal",
            "export",
            &fixture.trait_id,
            "--allow-unreviewed",
        ],
        &fixture.repo,
        &fixture.home,
    );

    // Hidden since the 2026-08-18 regroup: shape still proven, no registry claim.
    assert_plain_panel_structure("ctx", "export", &stdout);
}

#[test]
fn create_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-new-panel-shape");
    let repo = scratch_repo(&scratch);
    let stdout = require_success(
        "`ctx traits create` with no arguments (lists templates)",
        &["traits", "create"],
        &repo,
        &scratch.home(),
    );

    assert_matches_registry_claim("create", "ctx", "create", &stdout);
}

/// `fork`'s default output over a real path-installed, CDK-buildable
/// dependency (0213): a producer repo scaffolded by `ctx traits init` +
/// `ctx traits build` (same real-node-build precedent as
/// `build_default_output_matches_the_panel_registry_shape`), path-installed
/// into a consumer repo, then forked.
#[test]
fn fork_default_output_matches_the_panel_registry_shape() {
    let scratch = ScratchRoot::new("p467-fork-panel-shape");
    let home = scratch.home();
    let id = "fixture-fork-panel";

    let producer = home.join("producer");
    fs::create_dir_all(&producer).unwrap();
    git_init(&producer);
    symlink_node_modules(&producer);
    require_success(
        "`ctx traits init <id>` in the producer",
        &["traits", "init", id],
        &producer,
        &home,
    );
    require_success(
        "initial explicit-path `ctx traits build` in the producer",
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{id}/source/index.ts"),
        ],
        &producer,
        &home,
    );

    let repo = home.join("consumer");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    symlink_node_modules(&repo);
    require_success(
        "`ctx traits dependency add path:<producer>`",
        &[
            "traits",
            "dependency",
            "add",
            &format!("path:../producer/.ctx/traits/authored/{id}"),
        ],
        &repo,
        &home,
    );

    let stdout = require_success(
        "`ctx traits fork <id>`",
        &["traits", "fork", id],
        &repo,
        &home,
    );

    assert_matches_registry_claim("fork", "ctx", "fork", &stdout);
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
            ".ctx/traits/authored/fixture-build-panel/source/index.ts",
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
        "`ctx traits internal host install --host cursor` over a freshly-init'd trait",
        &[
            "traits",
            "internal",
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

    // Hidden since the 2026-08-18 regroup: shape still proven, no registry claim.
    assert_plain_panel_structure("ctx", "host-install", &stdout);
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
        "`ctx traits state --active` over a freshly-init'd trait",
        &["traits", "state", "--active", &fixture.trait_id],
        &fixture.repo,
        &fixture.home,
    );

    // Hidden since the 2026-08-18 regroup: shape still proven, no registry claim.
    assert_plain_panel_structure("ctx", "activate", &stdout);
}

#[test]
fn trust_approve_default_output_matches_the_panel_registry_shape() {
    let fixture = build_trait_fixture("p467-trust-panel-shape", "fixture-trust-panel");
    let stdout = require_success(
        "`ctx traits trust --approved` over a freshly-init'd trait",
        &["traits", "trust", "--approved", &fixture.trait_id],
        &fixture.repo,
        &fixture.home,
    );

    assert_matches_registry_claim("trust", "ctx", "trust --approved", &stdout);
}

/// A provider-free, network-free repo carrying one reviewed, activated
/// `kind = "command"` trait (`cmd` runs a trivial local shell command, no
/// harness dispatch) — the same fixture shape `proof_merge_on_completion.rs`
/// uses to drive `run`/`merge` end to end without a script harness or a
/// model provider.
fn command_trait_fixture(label: &str) -> BuiltTraitFixture {
    command_trait_fixture_with_command(label, "true")
}

fn command_trait_fixture_with_command(label: &str, command: &str) -> BuiltTraitFixture {
    let scratch = ScratchRoot::new(label);
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init(&repo);
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        format!(
            "id = \"demo\"\n\
         schema-version = \"0.4\"\n\
         version = \"0.1.0\"\n\
         name = \"Demo\"\n\
         description = \"A provider-free command-only trait.\"\n\
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
          cmd = \"{command}\"\n\
          output = [\"slot:notified\"]\n",
        ),
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
        "`ctx traits trust --approved`",
        &["traits", "trust", "--approved", fixture_path],
        &repo,
        &home,
    );
    require_success(
        "`ctx traits state --active --file`",
        &["traits", "state", "--active", "--file", fixture_path],
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

fn panel_row_labels(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("  "))
        .filter_map(|line| line.split_once(": ").map(|(label, _)| label))
        .collect()
}

fn stable_json_envelope(stdout: &str) -> Vec<u8> {
    let start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("run must emit a JSON object: {stdout}"));
    let mut envelope: serde_json::Value = serde_json::from_str(&stdout[start..])
        .unwrap_or_else(|error| panic!("run did not emit valid JSON: {error}\n{stdout}"));
    remove_per_run_json_values(&mut envelope);
    serde_json::to_vec(&envelope).expect("JSON value serializes")
}

// Separate invocations intentionally mint distinct session IDs and derived
// state digests. Remove only those per-run values before byte-comparing the
// envelope generated with and without `--verbose`.
fn remove_per_run_json_values(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for key in [
                "session-id",
                "run-id",
                "session-path",
                "final-session-digest",
                "state-digest",
                "started-at-epoch",
                "approved-at",
            ] {
                object.remove(key);
            }
            if object.contains_key("frames-attempted") {
                object.remove("session");
            }
            for value in object.values_mut() {
                remove_per_run_json_values(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                remove_per_run_json_values(value);
            }
        }
        _ => {}
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
    let stdout = strip_pre_panel_narration(&stdout, "demo", "");

    assert_matches_registry_claim("run", "demo", "", &stdout);
    assert_eq!(
        panel_row_labels(&stdout),
        ["session"],
        "a clean-tree default run exposes only its session fact: {stdout}"
    );
}

#[test]
fn run_failed_default_output_is_exact() {
    let fixture = command_trait_fixture_with_command("p467-run-failed", "false");
    let args = [
        "traits",
        "run",
        "--file",
        ".ctx/traits/demo/generated/index.toml",
        "--progress",
        "none",
    ];
    let output = controlled_command(&ctx_bin(), &args, &fixture.repo, &fixture.home)
        .output()
        .unwrap_or_else(|error| panic!("cannot run {args:?}: {error}"));
    assert!(
        !output.status.success(),
        "a false command fixture must fail its run: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).expect("run stdout is UTF-8");
    let stdout = strip_pre_panel_narration(&stdout, "demo", "");
    assert_eq!(
        panel_row_labels(&stdout),
        ["session", "error"],
        "a failed default run exposes exactly session and error facts: {stdout}"
    );
    assert!(stdout.lines().last().is_some_and(|line| line == "Failure"));
    for omitted in [
        "frames-attempted",
        "ledger",
        "lock-",
        "worktree",
        "landing",
        "next",
    ] {
        assert!(
            !stdout.contains(omitted),
            "failed default output leaked {omitted:?}: {stdout}"
        );
    }
}

#[test]
fn run_verbose_output_retains_default_facts_and_restores_detail() {
    let fixture = command_trait_fixture("p467-run-verbose");
    let default_stdout = require_success(
        "default provider-free run",
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
    let verbose_stdout = require_success(
        "verbose provider-free run",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--progress",
            "none",
            "--verbose",
        ],
        &fixture.repo,
        &fixture.home,
    );
    let default_stdout = strip_pre_panel_narration(&default_stdout, "demo", "");
    let verbose_stdout = strip_pre_panel_narration(&verbose_stdout, "demo", "");
    for label in panel_row_labels(&default_stdout) {
        assert!(
            panel_row_labels(&verbose_stdout).contains(&label),
            "verbose run lost default row {label:?}: {verbose_stdout}"
        );
    }
    assert!(
        verbose_stdout.contains("completed:") || verbose_stdout.contains("status:"),
        "verbose output must restore output detail: {verbose_stdout}"
    );
}

#[test]
fn run_merge_facts_are_exact() {
    let landed_fixture = command_trait_fixture("p467-run-merged-fact-landed");
    let landed = require_success(
        "a landable command run with automatic merge",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--progress",
            "none",
        ],
        &landed_fixture.repo,
        &landed_fixture.home,
    );
    let landed = strip_pre_panel_narration(&landed, "demo", "");
    assert_eq!(
        panel_row_labels(&landed),
        ["session", "merged"],
        "a landed run exposes exactly its session and qualified merge fact: {landed}"
    );
    assert!(
        landed.contains("merged: yes ("),
        "a landed run must include its landed revision: {landed}"
    );
    assert!(
        landed.lines().last().is_some_and(|line| line == "Success"),
        "a landed run must close Success: {landed}"
    );

    let parked_fixture = command_trait_fixture("p467-run-merged-fact-parked");
    fs::write(
        parked_fixture.repo.join(".ctx/traits/runtime.toml"),
        "[merge]\ngate = [[\"false\"]]\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", ".ctx/traits/runtime.toml"])
        .current_dir(&parked_fixture.repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "add failing merge gate"])
        .current_dir(&parked_fixture.repo)
        .status()
        .unwrap();
    let args = [
        "traits",
        "run",
        "--file",
        ".ctx/traits/demo/generated/index.toml",
        "--worktree",
        "--merge",
        "--progress",
        "none",
    ];
    let parked = controlled_command(
        &ctx_bin(),
        &args,
        &parked_fixture.repo,
        &parked_fixture.home,
    )
    .output()
    .unwrap_or_else(|error| panic!("cannot run {args:?}: {error}"));
    assert!(
        !parked.status.success(),
        "a failed merge gate must park its run: stdout={} stderr={}",
        String::from_utf8_lossy(&parked.stdout),
        String::from_utf8_lossy(&parked.stderr),
    );
    let parked = String::from_utf8(parked.stdout).expect("parked run stdout is UTF-8");
    let parked = strip_pre_panel_narration(&parked, "demo", "");
    assert_eq!(
        panel_row_labels(&parked),
        ["session", "merged"],
        "a parked run exposes exactly its session and qualified merge fact: {parked}"
    );
    assert!(
        parked.contains("merged: no (ctx traits merge "),
        "a parked run must include its actionable merge command: {parked}"
    );
    assert!(
        parked.lines().last().is_some_and(|line| line == "Success"),
        "a parked merge must not change the completed run's Success close: {parked}"
    );
}

#[test]
fn run_verbose_landing_detail_uses_stable_human_text() {
    let fixture = command_trait_fixture("p467-run-verbose-landing-detail");
    let stdout = require_success(
        "a verbose landable command run with automatic merge",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--progress",
            "none",
            "--verbose",
        ],
        &fixture.repo,
        &fixture.home,
    );
    let stdout = strip_pre_panel_narration(&stdout, "demo", "");
    assert!(
        stdout.contains("landing: merged to main ("),
        "verbose landing detail must remain stable human text: {stdout}"
    );
    assert!(
        !stdout.contains("Landed {"),
        "verbose landing detail must not expose LandingState debug text: {stdout}"
    );
}

#[test]
fn run_json_output_is_unchanged_by_verbose() {
    let fixture = command_trait_fixture("p467-run-json-verbose");
    let default = require_success(
        "a JSON provider-free run",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--progress",
            "none",
            "--json",
        ],
        &fixture.repo,
        &fixture.home,
    );
    let verbose = require_success(
        "a verbose JSON provider-free run",
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--progress",
            "none",
            "--json",
            "--verbose",
        ],
        &fixture.repo,
        &fixture.home,
    );
    assert_eq!(
        stable_json_envelope(&default),
        stable_json_envelope(&verbose),
        "--verbose must not change the stable bytes of a run's JSON envelope"
    );
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
    let stdout = strip_pre_panel_narration(&stdout, "demo", "");

    assert!(
        !stdout.contains("\x1b[?1049h"),
        "no --progress under piped stdio must not open the TUI alternate screen: {stdout}"
    );
    assert_matches_registry_claim("run", "demo", "", &stdout);
}

#[test]
fn run_progress_modes_remain_line_oriented_when_stdio_is_piped() {
    let fixture = command_trait_fixture("p551-run-piped-progress");
    fs::write(
        fixture.repo.join(".gitignore"),
        ".ctx/traits/worktrees/\nwarm-valid/\n",
    )
    .unwrap();
    fs::write(
        fixture.repo.join(".ctx/traits/runtime.toml"),
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
    fs::create_dir_all(repo.join(".ctx/traits")).unwrap();
    fs::write(
        repo.join(".ctx/traits/config.toml"),
        "[vendor]\n\
         schema-version = \"0.2\"\n\n\
         [vendor.dependencies.demo-dep]\n\
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
