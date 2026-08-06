//! Runs a task's declared `checks` (0144) against a clean checkout of a
//! merged sha, and maps each raw execution to a [`CheckRecord`] verdict.
//!
//! Verdict mapping is pure and separated from process execution (mirroring
//! `task_proposals.rs`'s style) so it unit-tests with literal
//! `CheckExecution` values, never a live command. The runner itself
//! (`run_checks`) is the one place this module touches the filesystem or
//! spawns a process.

use camino::Utf8PathBuf;
use ctx_traits_core::task::{Check, CheckOutcome, CheckRecord};

/// Hard cap on declared checks per task (0144's Files-to-touch #5): past
/// this, the whole set is un-runnable with a named reason rather than
/// silently truncated.
pub const MAX_CHECKS: usize = 8;

/// Default per-check wall-clock ceiling when a check declares none.
pub const DEFAULT_CHECK_TIMEOUT_MS: u64 = 120_000;

/// One check's raw execution result, before verdict mapping. Kept separate
/// from [`ctx_traits_io::command::RunOutput`] so [`verdict`] unit-tests with
/// plain literals instead of constructing a live process output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckExecution {
    pub exit_code: Option<i32>,
    pub combined_output: String,
    pub timed_out: bool,
}

/// Map one check's declaration and raw execution to its recorded verdict.
/// Pass requires exit 0 AND, when `expect` is declared, the regex matching
/// `combined_output`. A declared `expect` that matches nothing fails with
/// "expected output not found" — never implied as a stronger pass than a
/// bare exit code would give.
pub fn verdict(check: &Check, execution: &CheckExecution) -> CheckRecord {
    let record = |outcome, detail: String| CheckRecord {
        name: check.name.clone(),
        command: check.command.clone(),
        outcome,
        detail,
    };
    if execution.timed_out {
        return record(
            CheckOutcome::Unrunnable,
            "timed out before it could report a verdict".to_string(),
        );
    }
    if execution.exit_code != Some(0) {
        return record(
            CheckOutcome::Failed,
            match execution.exit_code {
                Some(code) => format!("exited {code}"),
                None => "terminated by signal".to_string(),
            },
        );
    }
    if let Some(expect) = &check.expect {
        let matches = match regex_lite::Regex::new(expect) {
            Ok(re) => re.is_match(&execution.combined_output),
            Err(e) => {
                return record(
                    CheckOutcome::Unrunnable,
                    format!("expect pattern {expect:?} does not compile: {e}"),
                );
            }
        };
        if !matches {
            return record(
                CheckOutcome::Failed,
                "expected output not found".to_string(),
            );
        }
        return record(CheckOutcome::Passed, "exit 0, expect matched".to_string());
    }
    record(CheckOutcome::Passed, "exit 0".to_string())
}

/// Why a task's whole check set could not run at all — distinct from any
/// individual check's outcome, since it applies before any check executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrunnableSet {
    pub reason: String,
}

/// Verdict for every declared check, applying the count cap before anything
/// executes. `Err` names why the WHOLE set is un-runnable (over the cap, or
/// the checkout/worktree step itself failed) — never a partial run.
pub fn run_checks(
    checks: &[Check],
    repo_root: &camino::Utf8Path,
    sha: &str,
) -> Result<Vec<CheckRecord>, UnrunnableSet> {
    if checks.is_empty() {
        return Ok(Vec::new());
    }
    if checks.len() > MAX_CHECKS {
        return Err(UnrunnableSet {
            reason: format!(
                "{} checks declared, exceeding the cap of {MAX_CHECKS} — refusing to run a \
                 truncated set",
                checks.len()
            ),
        });
    }

    let worktree_id = format!("task-check-{}", short_random_suffix(sha));
    let worktree_path = repo_root
        .parent()
        .unwrap_or(repo_root)
        .join(format!(".ctx-task-check-{worktree_id}"));

    let add = ctx_traits_io::git_process::run(ctx_traits_io::git_process::Request {
        exec_dir: Some(repo_root),
        cwd: None,
        args: &["worktree", "add", "--detach", worktree_path.as_str(), sha],
        success_exit_code: &[0],
        timeout_ms: ctx_traits_io::git_process::LONG_TIMEOUT_MS,
        capture_limit: 16_384,
    });
    let add = match add {
        Ok(output) if output.success => output,
        Ok(output) => {
            return Err(UnrunnableSet {
                reason: format!(
                    "git worktree add for {sha} failed: {}",
                    output.stderr.trim()
                ),
            });
        }
        Err(e) => {
            return Err(UnrunnableSet {
                reason: format!("git worktree add for {sha} failed: {e}"),
            });
        }
    };
    let _ = add;

    let records = checks
        .iter()
        .map(|check| execute_one(check, &worktree_path))
        .collect();

    let _ = ctx_traits_io::git_process::run(ctx_traits_io::git_process::Request {
        exec_dir: Some(repo_root),
        cwd: None,
        args: &["worktree", "remove", "--force", worktree_path.as_str()],
        success_exit_code: &[0],
        timeout_ms: ctx_traits_io::git_process::LONG_TIMEOUT_MS,
        capture_limit: 4096,
    });

    Ok(records)
}

fn execute_one(check: &Check, worktree_path: &Utf8PathBuf) -> CheckRecord {
    let timeout_ms = check.timeout_ms.unwrap_or(DEFAULT_CHECK_TIMEOUT_MS);
    let argv = vec!["sh".to_string(), "-c".to_string(), check.command.clone()];
    let mut env_overlay = std::collections::BTreeMap::new();
    // Cross-worktree CARGO_TARGET_DIR flake: isolate the build cache inside
    // this scratch checkout rather than sharing the caller's target dir.
    env_overlay.insert(
        "CARGO_TARGET_DIR".to_string(),
        worktree_path.join("target").to_string(),
    );
    let request = ctx_traits_io::command::RunRequest {
        argv: &argv,
        cwd: None,
        exec_dir: Some(worktree_path.as_path()),
        success_exit_code: &[0],
        timeout_ms: Some(timeout_ms),
        idle_timeout_ms: Some(timeout_ms),
        capture_limit: 65_536,
        tick_observer: None,
    };
    match ctx_traits_io::command::run_with_env(request, &env_overlay) {
        Ok(output) => verdict(
            check,
            &CheckExecution {
                exit_code: output.exit_code,
                combined_output: format!("{}{}", output.stdout, output.stderr),
                timed_out: output.timed_out,
            },
        ),
        Err(e) => CheckRecord {
            name: check.name.clone(),
            command: check.command.clone(),
            outcome: CheckOutcome::Unrunnable,
            detail: format!("could not execute: {e}"),
        },
    }
}

/// A short, deterministic-enough discriminator for the scratch worktree
/// directory name — collision only matters within one concurrent run, and
/// this repo's `Date.now()`/`Math.random()` prohibition does not apply to
/// plain Rust code, but a hash of the sha is simpler than reaching for a
/// counter here.
fn short_random_suffix(sha: &str) -> String {
    sha.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, command: &str, expect: Option<&str>) -> Check {
        Check {
            name: name.to_string(),
            command: command.to_string(),
            timeout_ms: None,
            expect: expect.map(str::to_string),
        }
    }

    fn execution(exit_code: Option<i32>, output: &str, timed_out: bool) -> CheckExecution {
        CheckExecution {
            exit_code,
            combined_output: output.to_string(),
            timed_out,
        }
    }

    #[test]
    fn zero_exit_with_no_expect_passes() {
        let c = check("t", "true", None);
        let record = verdict(&c, &execution(Some(0), "", false));
        assert_eq!(record.outcome, CheckOutcome::Passed);
    }

    #[test]
    fn nonzero_exit_fails_regardless_of_expect() {
        let c = check("t", "false", Some("ok"));
        let record = verdict(&c, &execution(Some(1), "ok", false));
        assert_eq!(record.outcome, CheckOutcome::Failed);
        assert!(record.detail.contains('1'));
    }

    #[test]
    fn zero_exit_with_matching_expect_passes() {
        let c = check("t", "cargo test", Some("test result: ok"));
        let record = verdict(
            &c,
            &execution(Some(0), "running 3 tests\ntest result: ok. 3 passed", false),
        );
        assert_eq!(record.outcome, CheckOutcome::Passed);
    }

    #[test]
    fn zero_exit_with_expect_matching_nothing_fails_as_vacuous() {
        let c = check("t", "cargo test -- filter", Some("test result: ok"));
        let record = verdict(&c, &execution(Some(0), "running 0 tests", false));
        assert_eq!(record.outcome, CheckOutcome::Failed);
        assert_eq!(record.detail, "expected output not found");
    }

    #[test]
    fn timeout_is_unrunnable_never_pass_or_fail() {
        let c = check("t", "sleep 999", None);
        let record = verdict(&c, &execution(None, "", true));
        assert_eq!(record.outcome, CheckOutcome::Unrunnable);
    }

    #[test]
    fn signal_termination_without_timeout_is_failed_not_unrunnable() {
        let c = check("t", "kill -9 $$", None);
        let record = verdict(&c, &execution(None, "", false));
        assert_eq!(record.outcome, CheckOutcome::Failed);
    }

    #[test]
    fn invalid_expect_pattern_is_unrunnable() {
        let c = check("t", "true", Some("(unclosed"));
        let record = verdict(&c, &execution(Some(0), "", false));
        assert_eq!(record.outcome, CheckOutcome::Unrunnable);
    }

    #[test]
    fn over_cap_check_count_refuses_the_whole_set_before_running_any() {
        let repo = camino::Utf8PathBuf::from("/does/not/matter");
        let checks: Vec<Check> = (0..MAX_CHECKS + 1)
            .map(|i| check(&format!("c{i}"), "true", None))
            .collect();
        let result = run_checks(&checks, &repo, "HEAD");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .reason
                .contains(&(MAX_CHECKS + 1).to_string())
        );
    }

    #[test]
    fn empty_checks_run_to_an_empty_result_without_touching_git() {
        let repo = camino::Utf8PathBuf::from("/does/not/matter");
        assert_eq!(run_checks(&[], &repo, "HEAD"), Ok(Vec::new()));
    }

    /// Integration: a real scratch git repo, running `true`, `false`, and a
    /// sleep exceeding a tiny timeout, through the actual worktree +
    /// process-execution path.
    fn tempdir() -> Utf8PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "task-checks-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Utf8PathBuf::from_path_buf(dir).unwrap()
    }

    #[test]
    fn runner_against_a_scratch_repo_yields_pass_fail_and_unrunnable() {
        let repo_root = tempdir();
        let repo_root = repo_root.as_path();
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo_root.as_std_path())
                .status()
                .expect("git runs")
        };
        assert!(run_git(&["init", "-q"]).success());
        assert!(run_git(&["config", "user.email", "t@example.com"]).success());
        assert!(run_git(&["config", "user.name", "t"]).success());
        std::fs::write(repo_root.join("f.txt"), "hello\n").unwrap();
        assert!(run_git(&["add", "."]).success());
        assert!(run_git(&["commit", "-q", "-m", "init"]).success());

        let checks = vec![
            check("passes", "true", None),
            check("fails", "false", None),
            Check {
                name: "hangs".to_string(),
                command: "sleep 5".to_string(),
                timeout_ms: Some(200),
                expect: None,
            },
        ];
        let records = run_checks(&checks, repo_root, "HEAD").expect("set runs");
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].outcome, CheckOutcome::Passed);
        assert_eq!(records[1].outcome, CheckOutcome::Failed);
        assert_eq!(records[2].outcome, CheckOutcome::Unrunnable);
    }
}
