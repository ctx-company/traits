//! Public-path proofs for P466's compact `ctx traits doctor` panel: the
//! default source-inspection report migrated to the P465 scrollback
//! presentation kit. Structural assertions on real process output, no
//! golden/snapshot fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use support::{ScratchRoot, assert_exit_code, git_init, require_success, run_ctx, utf8};

/// P491: `doctor` now exits `EXIT_FINDINGS` (6) when it ran and found a
/// critical finding, distinct from exit 0 (no findings) and exit 1 (could
/// not run). Several proofs below intentionally produce a critical finding,
/// so they read stdout via this helper (which tolerates the non-zero exit)
/// rather than [`require_success`].
fn stdout_allowing_findings_exit(args: &[&str], cwd: &Path, home: &Path) -> String {
    let output = run_ctx(args, cwd, home);
    let (stdout, stderr) = utf8(&output);
    assert!(
        matches!(output.status.code(), Some(0) | Some(6)),
        "expected exit 0 or 6, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code()
    );
    stdout
}

/// A dedicated, empty Git repository under `scratch`, used as every test's
/// `cwd` instead of the real repository root: doctor's cross-tier
/// trait-shadow resolution only consults the project tier inside a genuine
/// Git repository, so this keeps each test's output independent of
/// whatever traits happen to live in the actual working checkout.
///
/// Pre-seeds a complete nested `.ctx/.gitignore` (P446), matching an
/// already-onboarded repository, so these tests' compact-panel/exit-code
/// assertions stay about source-inspection findings rather than repo-state
/// housekeeping — that diagnostic has its own dedicated coverage in
/// `scripts/byte_compare.rs --state-proof`.
fn scratch_repo(scratch: &ScratchRoot) -> PathBuf {
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let gitignore_dir = repo.join(".ctx");
    fs::create_dir_all(&gitignore_dir).unwrap();
    fs::write(
        gitignore_dir.join(".gitignore"),
        "worktrees/\nconfig.toml\nconfig.ts\nharness.toml\ntraits/vendor/\nruns/\ndebug/\ncache/\n",
    )
    .unwrap();
    repo
}

/// A healthy `SKILL.md` with no hidden-content findings.
fn write_healthy_skill(dir: &Path, relative: &str) {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "---\nname: Healthy Skill\ndescription: A demo skill with no findings.\n---\n\n\
         # Healthy Skill\n\nNothing hidden here.\n",
    )
    .unwrap();
}

/// A `SKILL.md` with one critical (HTML comment) and one warning
/// (color-on-color) hidden-content finding.
fn write_broken_skill(dir: &Path, relative: &str) {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "---\nname: Broken Skill\ndescription: A demo skill with hidden content.\n---\n\n\
         # Broken Skill\n\n\
         <!-- secret instructions for the agent -->\n\n\
         <span style=\"color:#fff;background-color:#fff\">invisible</span>\n",
    )
    .unwrap();
}

/// A `SKILL.md`-named file with invalid UTF-8 bytes, so discovery's read
/// fails and the candidate surfaces as a read/plan failure instead of an
/// analyzed entry.
fn write_unreadable_skill(dir: &Path, relative: &str) {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, [0xFFu8, 0xFE, b'x']).unwrap();
}

#[test]
fn compact_output_leads_with_counts_and_omits_healthy() {
    let scratch = ScratchRoot::new("p466-compact-counts");
    let root = scratch.home().join("sources");
    write_healthy_skill(&root, "healthy/SKILL.md");
    write_broken_skill(&root, "broken/SKILL.md");
    write_unreadable_skill(&root, "unread/SKILL.md");

    let repo = scratch_repo(&scratch);
    let stdout = stdout_allowing_findings_exit(
        &["traits", "doctor", root.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "ctx doctor", "{stdout}");
    assert!(lines[1].contains("checks: 4"), "{stdout}");
    assert!(stdout.contains("passed: 1"), "{stdout}");
    assert!(stdout.contains("warnings: 1"), "{stdout}");
    assert!(stdout.contains("critical: 2"), "{stdout}");

    assert!(
        !stdout.contains("healthy/SKILL.md"),
        "healthy candidate must be omitted: {stdout}"
    );
    assert!(stdout.contains("broken/SKILL.md"), "{stdout}");
    assert!(stdout.contains("unread/SKILL.md"), "{stdout}");
    // Verbose-only per-candidate detail (raw-source-digest) must not leak
    // into the compact panel.
    assert!(!stdout.contains("raw-source-digest"), "{stdout}");
}

#[test]
fn compact_output_names_broken_candidates_with_concrete_fix_text() {
    let scratch = ScratchRoot::new("p466-compact-fix-text");
    let root = scratch.home().join("sources");
    write_broken_skill(&root, "broken/SKILL.md");
    write_unreadable_skill(&root, "unread/SKILL.md");

    let repo = scratch_repo(&scratch);
    let stdout = stdout_allowing_findings_exit(
        &["traits", "doctor", root.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );

    assert!(stdout.contains("remove HTML comments"), "{stdout}");
    assert!(
        stdout.contains("avoid same-color foreground/background"),
        "{stdout}"
    );
    assert!(
        stdout.contains("repair unread/SKILL.md and rerun"),
        "{stdout}"
    );
}

#[test]
fn compact_output_is_plain_with_no_ansi_or_box_glyphs_when_piped() {
    let scratch = ScratchRoot::new("p466-compact-plain");
    let root = scratch.home().join("sources");
    write_healthy_skill(&root, "healthy/SKILL.md");

    let repo = scratch_repo(&scratch);
    let stdout = require_success(
        "`ctx traits doctor` over a healthy source tree",
        &["traits", "doctor", root.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );

    for glyph in ["╭", "╰", "│", "─", "\x1b["] {
        assert!(
            !stdout.contains(glyph),
            "must not contain {glyph:?}: {stdout}"
        );
    }
}

#[test]
fn healthy_case_fits_a_conservative_one_screen_line_bound() {
    let scratch = ScratchRoot::new("p466-compact-bound");
    let root = scratch.home().join("sources");
    write_healthy_skill(&root, "healthy/SKILL.md");

    let repo = scratch_repo(&scratch);
    let stdout = require_success(
        "`ctx traits doctor` over a healthy source tree",
        &["traits", "doctor", root.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );

    let line_count = stdout.lines().count();
    assert!(
        line_count <= 12,
        "healthy compact output should fit well within one screen, got {line_count} lines: {stdout}"
    );
}

#[test]
fn verbose_restores_existing_detail_after_the_panel_exactly_once() {
    let scratch = ScratchRoot::new("p466-verbose-detail");
    let root = scratch.home().join("sources");
    write_broken_skill(&root, "broken/SKILL.md");

    let repo = scratch_repo(&scratch);
    let stdout = stdout_allowing_findings_exit(
        &["traits", "doctor", root.to_str().unwrap(), "--verbose"],
        &repo,
        &scratch.home(),
    );

    assert!(stdout.contains("checks: 2"), "{stdout}");
    // Full per-candidate narrative, unique to the existing detailed
    // renderer, appears exactly once.
    let digest_occurrences = stdout.matches("raw-source-digest:").count();
    assert_eq!(digest_occurrences, 1, "{stdout}");
    let files_summary_occurrences = stdout.matches("files: total=").count();
    assert_eq!(files_summary_occurrences, 1, "{stdout}");

    // The panel comes first, the full narrative after it.
    let panel_index = stdout.find("checks: 2").unwrap();
    let detail_index = stdout.find("raw-source-digest:").unwrap();
    assert!(panel_index < detail_index, "{stdout}");
}

#[test]
fn json_and_verbose_json_are_byte_equal_and_contain_no_panel_framing() {
    let scratch = ScratchRoot::new("p466-json-equivalence");
    let root = scratch.home().join("sources");
    write_broken_skill(&root, "broken/SKILL.md");
    write_unreadable_skill(&root, "unread/SKILL.md");

    let repo = scratch_repo(&scratch);
    let json_stdout = stdout_allowing_findings_exit(
        &["traits", "doctor", root.to_str().unwrap(), "--json"],
        &repo,
        &scratch.home(),
    );
    let verbose_json_stdout = stdout_allowing_findings_exit(
        &[
            "traits",
            "doctor",
            root.to_str().unwrap(),
            "--verbose",
            "--json",
        ],
        &repo,
        &scratch.home(),
    );

    assert_eq!(
        json_stdout, verbose_json_stdout,
        "--json must win over --verbose"
    );
    assert!(!json_stdout.contains("checks:"), "{json_stdout}");
    assert!(!json_stdout.contains("╭"), "{json_stdout}");
    assert!(serde_json::from_str::<serde_json::Value>(&json_stdout).is_ok());
}

#[test]
fn verbose_conflicts_with_config_and_migrate_state() {
    let scratch = ScratchRoot::new("p466-verbose-conflicts");
    let repo = scratch_repo(&scratch);

    let config_output = run_ctx(
        &["traits", "doctor", "--config", "--verbose"],
        &repo,
        &scratch.home(),
    );
    assert_ne!(config_output.status.code(), Some(0));

    let migrate_output = run_ctx(
        &["traits", "doctor", "--migrate-state", "--verbose"],
        &repo,
        &scratch.home(),
    );
    assert_ne!(migrate_output.status.code(), Some(0));
}

#[test]
fn healthy_invocation_exits_zero() {
    let scratch = ScratchRoot::new("p466-exit-codes-ok");
    let root = scratch.home().join("sources");
    write_healthy_skill(&root, "healthy/SKILL.md");

    let repo = scratch_repo(&scratch);
    let output = run_ctx(
        &["traits", "doctor", root.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);
}

/// P491: a critical hidden-content finding is now a typed, distinct exit
/// (6) — the command ran to completion and reports a blocking finding,
/// which is neither "could not run" (1) nor "nothing to report" (0).
#[test]
fn critical_finding_invocation_exits_findings_code() {
    let scratch = ScratchRoot::new("p491-doctor-exit-findings");
    let root = scratch.home().join("sources");
    write_healthy_skill(&root, "healthy/SKILL.md");
    write_broken_skill(&root, "broken/SKILL.md");

    let repo = scratch_repo(&scratch);
    let output = run_ctx(
        &["traits", "doctor", root.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 6);
}

#[test]
fn missing_source_still_exits_nonzero() {
    let scratch = ScratchRoot::new("p466-exit-codes-missing");
    let root = scratch.home().join("empty-sources");
    fs::create_dir_all(&root).unwrap();
    let repo = scratch_repo(&scratch);

    let output = run_ctx(
        &["traits", "doctor", root.to_str().unwrap()],
        &repo,
        &scratch.home(),
    );
    assert_ne!(output.status.code(), Some(0));
}
