//! 0163: scratch-fixture behavioral proofs for the `optimize` family's
//! deterministic keep/discard guard, N-run median aggregation, and
//! immutable-baseline/reset machinery — the crown-jewel mechanism this
//! package exists to carry over faithfully from the recovered
//! `auto-research` package.
//!
//! Rather than hand-authoring a reduced mechanism (research_family's own
//! precedent), this suite reads the ACTUAL built canonical
//! (`.ctx/traits/authored/optimize/generated/experiment/index.toml`, and
//! the `benchmark` variant) at test-run time and drives it end to end under
//! `ctx traits run` in a disposable scratch repository — the real guard
//! code the round ships, not a paraphrase of it. Only the trait `id` is
//! rewritten (to avoid colliding with the real, separately-activated
//! `optimize` package); every step, condition, and script is untouched. The
//! caller-supplied `experiment-command`/`benchmark-command` argv points at
//! a fixed queue-driven Node script written into the scratch repo, so each
//! scenario controls exactly what every baseline/candidate run reports.

use std::fs;
use std::path::Path;

use support::{
    ScratchRoot, fixture_agent_bin, git_init, repo_root, require_success_with_env, run_git, utf8,
};

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("cannot create directory {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

fn commit_all(repo: &Path, home: &Path, message: &str) {
    for args in [vec!["add", "-A"], vec!["commit", "--quiet", "-m", message]] {
        let output = run_git(&args, repo, home);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn git_rev_parse_head(repo: &Path, home: &Path) -> String {
    let output = run_git(&["rev-parse", "HEAD"], repo, home);
    assert!(output.status.success(), "git rev-parse HEAD failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Asserts `ancestor` is still reachable from the repo's current `HEAD` —
/// i.e. the immutable baseline commit captured before any round was never
/// rewritten or discarded, whether the round that followed was kept
/// (HEAD moves forward past it) or reset (HEAD lands back on it exactly).
fn assert_baseline_still_ancestor(repo: &Path, home: &Path, ancestor: &str) {
    let output = run_git(
        &["merge-base", "--is-ancestor", ancestor, "HEAD"],
        repo,
        home,
    );
    assert!(
        output.status.success(),
        "baseline commit {ancestor} must remain an ancestor of HEAD (immutable baseline): {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Parses `ctx traits run --json`'s stdout down to the decoded
/// `port:summary` typed output value, read off
/// `value.session.completion.final-outputs[].value` for the entry whose
/// `port-ref` is `"port:summary"`.
fn summary_json(stdout: &str) -> serde_json::Value {
    let envelope: serde_json::Value = serde_json::from_str(stdout).unwrap_or_else(|error| {
        panic!("stdout was not a JSON envelope: {error}\nstdout:\n{stdout}")
    });
    let final_outputs = envelope
        .pointer("/value/session/completion/final-outputs")
        .and_then(|value| value.as_array())
        .unwrap_or_else(|| {
            panic!("no value/session/completion/final-outputs array in envelope:\n{envelope}")
        });
    final_outputs
        .iter()
        .find(|entry| entry.get("port-ref").and_then(|v| v.as_str()) == Some("port:summary"))
        .and_then(|entry| entry.get("value"))
        .unwrap_or_else(|| panic!("no port:summary entry in final-outputs:\n{envelope}"))
        .clone()
}

/// Reads the real built canonical for `variant` (`experiment`/`benchmark`)
/// and rewrites only its top-level `id` line, so the copy activates
/// independently of the real `optimize` package.
fn real_generated_canonical(variant: &str, fixture_id: &str) -> String {
    let path = repo_root().join(format!(
        ".ctx/traits/authored/optimize/generated/{variant}/index.toml"
    ));
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "cannot read real generated canonical {}: {error} (run `ctx traits build` first)",
            path.display()
        )
    });
    let rewritten = text.replacen(
        "id = \"optimize\"\n",
        &format!("id = \"{fixture_id}\"\n"),
        1,
    );
    assert_ne!(
        text,
        rewritten,
        "expected exactly one top-level `id = \"optimize\"` line to rewrite in {}",
        path.display()
    );
    rewritten
}

/// Every role bound to `ctx-fixture-agent`: `worker` (readiness/apply
/// /implement, dispatching on the requested output field), `proposer`
/// (experiment's fixed valid proposal), and `smart1` (benchmark's
/// scope/draft text plus its review verdict).
fn write_fixture_ctx_toml(repo: &Path) {
    let agent = fixture_agent_bin()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ctx_toml = format!(
        r#"schema-version = "0.4"

[harness.stub-worker]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-worker.cli]
argv = ["--role", "worker"]
output = "raw-json"

[harness.stub-proposer]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-proposer.cli]
argv = ["--role", "proposer"]
output = "raw-json"

[harness.stub-smart1]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-smart1.cli]
argv = ["--role", "smart1"]
output = "raw-json"

[agent.role.worker]
harness = "stub-worker"
transport = "cli"
session-mode = "per-frame"

[agent.role.proposer]
harness = "stub-proposer"
transport = "cli"
session-mode = "per-frame"

[agent.role.smart-1]
harness = "stub-smart1"
transport = "cli"
session-mode = "per-frame"
"#
    );
    write_file(&repo.join(".ctx/traits/runtime.toml"), &ctx_toml);
}

/// The queue-driven fake measurement command: pops the next `{status,metric
/// [,"delta-lines"]}` entry from `queue.json` on every invocation (one call
/// per aggregate run), cycling (`idx % queue.length`) once exhausted — the
/// real `optimize:experiment` canonical has no early-exit target, so its
/// bounded loop always runs its full 20-iteration budget, and a short,
/// repeating queue keeps every scenario's fixture compact while staying
/// fully deterministic and inspectable per invocation.
const MEASURE_SCRIPT: &str = r#"import { readFileSync, writeFileSync, existsSync } from "node:fs";
const queue = JSON.parse(readFileSync("queue.json", "utf8"));
const idxFile = "queue-index.txt";
const idx = existsSync(idxFile) ? Number(readFileSync(idxFile, "utf8")) : 0;
writeFileSync(idxFile, String(idx + 1));
const entry = queue[idx % queue.length];
process.stdout.write(JSON.stringify(entry));
"#;

/// Commits `measure.js`/`queue.json` so a discarded round's `git clean -fd`
/// (untracked-only) never deletes the measurement fixture itself — only
/// the worker's own untracked marker file is meant to be swept.
fn write_queue(repo: &Path, home: &Path, entries: &str) {
    write_file(&repo.join("measure.js"), MEASURE_SCRIPT);
    write_file(&repo.join("queue.json"), entries);
    commit_all(repo, home, "add measurement fixture");
}

fn package_toml(id: &str, name: &str) -> String {
    format!(
        "[package]\nid = \"{id}\"\nversion = \"0.1.0\"\nname = \"{name}\"\nstatus = \"draft\"\n"
    )
}

fn setup_fixture(
    label: &str,
    variant: &str,
    fixture_id: &str,
) -> (ScratchRoot, std::path::PathBuf, std::path::PathBuf) {
    let scratch = ScratchRoot::new(label);
    let repo = scratch.path().join("repo");
    let home = scratch.home();
    fs::create_dir_all(&repo)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", repo.display()));
    git_init(&repo);
    write_file(
        &repo.join("README.md"),
        "0163 optimize-family proof scratch repository\n",
    );
    write_file(
        &repo.join(".gitignore"),
        "/.ctx/runs/*.json\n/.ctx/traits/worktrees/\n/.ctx/debug/\n",
    );
    commit_all(&repo, &home, "initial commit");

    support::require_success("`ctx traits init`", &["traits", "init"], &repo, &home);

    write_file(
        &repo.join(format!(".ctx/traits/authored/{fixture_id}/trait.toml")),
        &package_toml(fixture_id, "Optimize Fixture"),
    );
    write_file(
        &repo.join(format!(
            ".ctx/traits/authored/{fixture_id}/generated/index.toml"
        )),
        &real_generated_canonical(variant, fixture_id),
    );
    write_fixture_ctx_toml(&repo);
    commit_all(&repo, &home, "add fixture trait");

    support::require_success(
        "`ctx traits check`",
        &["traits", "check", fixture_id, "--skip-cdk-drift", "--json"],
        &repo,
        &home,
    );
    support::require_success(
        "`ctx traits internal review --approve`",
        &["traits", "internal", "review", fixture_id, "--approve"],
        &repo,
        &home,
    );
    support::require_success(
        "`ctx traits internal state --active`",
        &["traits", "internal", "state", "--active", fixture_id],
        &repo,
        &home,
    );
    commit_all(&repo, &home, "activate fixture trait");

    (scratch, repo, home)
}

fn run_experiment(
    repo: &Path,
    home: &Path,
    fixture_id: &str,
    benchmark_runs: u32,
    max_delta_lines: Option<i64>,
) -> String {
    let mut input = format!(
        "{{\"port:objective\":\"fixture objective\",\"port:metric-field\":\"fixture metric\",\"port:benchmark-runs\":{benchmark_runs},\"port:experiment-command\":[\"node\",\"measure.js\"]"
    );
    if let Some(cap) = max_delta_lines {
        input.push_str(&format!(",\"port:max-delta-lines\":{cap}"));
    }
    input.push('}');
    write_file(&repo.join("input.json"), &input);
    // The real canonical's experiment loop always runs its full 20-round
    // budget (no early-exit target — see `iteration_cap_parks_...`), so one
    // invocation needs enough frame/time budget to drive baseline capture
    // plus 20 full propose/apply/measure/decide rounds to completion.
    require_success_with_env(
        "`ctx traits run` (optimize:experiment fixture)",
        &[
            "traits",
            "run",
            fixture_id,
            "--input",
            "input.json",
            "--max-frames",
            "400",
            "--frame-seconds",
            "30",
            "--total-seconds",
            "300",
            "--json",
        ],
        repo,
        home,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn run_benchmark(
    repo: &Path,
    home: &Path,
    fixture_id: &str,
    benchmark_runs: u32,
    improvement_target: f64,
    noise_threshold: f64,
    max_delta_lines: Option<i64>,
    reviewer_mode: &str,
) -> String {
    let mut input = format!(
        "{{\"port:target\":\"fixture-target\",\"port:benchmark-runs\":{benchmark_runs},\"port:improvement-target\":{improvement_target},\"port:noise-threshold\":{noise_threshold},\"port:benchmark-command\":[\"node\",\"measure.js\"]"
    );
    if let Some(cap) = max_delta_lines {
        input.push_str(&format!(",\"port:max-delta-lines\":{cap}"));
    }
    input.push('}');
    write_file(&repo.join("input.json"), &input);
    // `optimize:benchmark`'s round loop is capped at 12 iterations
    // (`loop.maxIterations(12, ...)`); every scenario here sets an
    // unreachable `improvement-target` so the run always drives its full
    // budget, mirroring `run_experiment`'s always-run design.
    require_success_with_env(
        "`ctx traits run` (optimize:benchmark fixture)",
        &[
            "traits",
            "run",
            fixture_id,
            "--input",
            "input.json",
            "--max-frames",
            "400",
            "--frame-seconds",
            "30",
            "--total-seconds",
            "300",
            "--json",
        ],
        repo,
        home,
        &[("CTX_FIXTURE_REVIEWER1_MODE", reviewer_mode)],
    )
}

/// review-gate routing (approved arm) + keep-on-improvement: an approved
/// candidate whose median clears both the keep guard and the noise
/// threshold is measured, kept, committed, and `bestRef` advances — proving
/// review approval only routes INTO measurement, never decides keep itself.
#[test]
fn benchmark_approved_review_routes_into_measurement_and_keeps_improvement() {
    let (scratch, repo, home) = setup_fixture(
        "p163-bench-keep",
        "benchmark",
        "optimize-fixture-bench-keep",
    );
    write_queue(
        &repo,
        &home,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":1},{"status":"ok","metric":1},{"status":"ok","metric":1}]"#,
    );
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = run_benchmark(
        &repo,
        &home,
        "optimize-fixture-bench-keep",
        3,
        -1000.0,
        0.5,
        None,
        "approve",
    );
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "run must complete:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_ne!(
        head_before, head_after,
        "an approved, noise-clearing improvement must be committed"
    );
    let summary = summary_json(&stdout);
    assert!(
        summary
            .get("rows")
            .and_then(|rows| rows.as_array())
            .into_iter()
            .flatten()
            .any(|row| row.get("decision").and_then(|d| d.as_str()) == Some("kept")),
        "summary rows must contain a kept round: {summary}"
    );
    assert_baseline_still_ancestor(&repo, &home, &head_before);

    let _ = scratch;
}

/// review-gate routing (revise arm): a `revise` verdict must revert the
/// candidate and record `review-rejected` without the runtime ever
/// consulting the measured-aggregate keep guard — the guard-authority
/// surface the draft names as the defect class to watch.
#[test]
fn benchmark_revise_review_reverts_and_records_review_rejected() {
    let (scratch, repo, home) = setup_fixture(
        "p163-bench-revise",
        "benchmark",
        "optimize-fixture-bench-revise",
    );
    // Every scripted run would improve on baseline if ever measured; the
    // revise verdict must still discard it without measuring.
    write_queue(
        &repo,
        &home,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":1},{"status":"ok","metric":1},{"status":"ok","metric":1}]"#,
    );
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = run_benchmark(
        &repo,
        &home,
        "optimize-fixture-bench-revise",
        3,
        -1000.0,
        0.5,
        None,
        "revise",
    );
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "run must complete:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before, head_after,
        "a revise verdict must never be committed, regardless of any measured improvement"
    );
    let summary = summary_json(&stdout);
    let rows = summary
        .get("rows")
        .and_then(|rows| rows.as_array())
        .unwrap_or_else(|| panic!("summary has no rows array: {summary}"));
    assert!(
        rows.iter()
            .any(|row| row.get("decision").and_then(|d| d.as_str()) == Some("review-rejected")),
        "summary rows must record review-rejected rounds: {summary}"
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.get("decision").and_then(|d| d.as_str()) == Some("kept")),
        "a revise verdict must never route into a kept decision: {summary}"
    );
    assert_baseline_still_ancestor(&repo, &home, &head_before);

    let _ = scratch;
}

/// discard-on-noise: an approved candidate whose median improves on best
/// but by less than `noise-threshold` is discarded, not kept — proving the
/// noise-margin guard, not just the raw `metric < bestMetric` comparison,
/// gates the benchmark variant's keep decision.
#[test]
fn benchmark_discard_on_noise_within_threshold() {
    let (scratch, repo, home) = setup_fixture(
        "p163-bench-noise",
        "benchmark",
        "optimize-fixture-bench-noise",
    );
    // Baseline median 10; candidate median 9 improves by only 1, which
    // never clears a noise threshold of 5.
    write_queue(
        &repo,
        &home,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":9},{"status":"ok","metric":9},{"status":"ok","metric":9}]"#,
    );
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = run_benchmark(
        &repo,
        &home,
        "optimize-fixture-bench-noise",
        3,
        -1000.0,
        5.0,
        None,
        "approve",
    );
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "run must complete:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before, head_after,
        "an improvement within the noise threshold must be discarded, never committed"
    );
    let summary = summary_json(&stdout);
    let rows = summary
        .get("rows")
        .and_then(|rows| rows.as_array())
        .unwrap_or_else(|| panic!("summary has no rows array: {summary}"));
    assert!(
        rows.iter()
            .any(|row| row.get("decision").and_then(|d| d.as_str()) == Some("discarded")),
        "summary rows must record noise-discarded rounds: {summary}"
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.get("decision").and_then(|d| d.as_str()) == Some("kept")),
        "an improvement within the noise threshold must never be kept: {summary}"
    );

    let _ = scratch;
}

/// max-delta-guard-severed regression proof, benchmark variant: an approved
/// candidate that clears both the keep guard and the noise threshold but
/// supplies no `delta-lines` is still discarded once `max-delta-lines` is
/// set — the shared `capGuard`/`maxDeltaGuard` composes identically into
/// both variants' keep guard.
#[test]
fn benchmark_max_delta_cap_discards_unmeasured_candidate() {
    let (scratch, repo, home) = setup_fixture(
        "p163-bench-max-delta",
        "benchmark",
        "optimize-fixture-bench-max-delta",
    );
    write_queue(
        &repo,
        &home,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":1},{"status":"ok","metric":1},{"status":"ok","metric":1}]"#,
    );
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = run_benchmark(
        &repo,
        &home,
        "optimize-fixture-bench-max-delta",
        3,
        -1000.0,
        0.5,
        Some(50),
        "approve",
    );
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "run must complete:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before, head_after,
        "an approved, noise-clearing candidate with no delta-lines must still be discarded once max-delta-lines is set"
    );

    let _ = scratch;
}

/// keep-on-improvement + aggregation-surfaces-runs: baseline's 3 runs
/// (10,10,10 -> median 10) seed best; round 1's 3 runs (7,5,6 -> median 6,
/// an actually-measured run, not the mean 6.0 of a differently-ordered set)
/// improve on best, so the candidate is committed and `bestRef` advances.
#[test]
fn keep_on_improvement_commits_and_surfaces_every_run() {
    let (scratch, repo, home) = setup_fixture("p163-keep", "experiment", "optimize-fixture-keep");
    write_queue(
        &repo,
        &home,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":7},{"status":"ok","metric":5},{"status":"ok","metric":6}]"#,
    );
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = run_experiment(&repo, &home, "optimize-fixture-keep", 3, None);
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "iteration-capped experiment always completes:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_ne!(
        head_before, head_after,
        "an improving candidate must be committed"
    );
    let log = run_git(&["log", "-1", "--format=%s"], &repo, &home);
    let (subject, _) = utf8(&log);
    assert!(
        subject.contains("10") && subject.contains("6"),
        "kept commit message must cite before (10) and after (6) metrics: {subject}"
    );
    assert!(
        subject.contains("3 runs"),
        "kept commit message must cite the run count: {subject}"
    );

    // aggregation-surfaces-runs: the kept round's three scripted per-run
    // metrics (7, 5, 6) must all survive into the typed summary's evidence
    // rows, not just the median (6) that reached the commit message.
    let summary = summary_json(&stdout);
    let kept_row = summary
        .get("rows")
        .and_then(|rows| rows.as_array())
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("decision").and_then(|d| d.as_str()) == Some("kept"))
        })
        .unwrap_or_else(|| panic!("no kept row in summary rows:\n{summary}"));
    let runs: Vec<i64> = kept_row
        .pointer("/measurement/runs")
        .and_then(|value| value.as_array())
        .unwrap_or_else(|| panic!("kept row has no measurement.runs array:\n{kept_row}"))
        .iter()
        .map(|value| value.as_i64().expect("run metric must be a number"))
        .collect();
    assert_eq!(
        runs,
        vec![7, 5, 6],
        "the kept round's summary row must surface every scripted per-run value, not only the median: {kept_row}"
    );

    // baseline immutability (kept-round case): the initial baseline commit,
    // captured before any round ran, must remain reachable from HEAD after
    // a round that advances HEAD forward via a kept commit.
    assert_baseline_still_ancestor(&repo, &home, &head_before);

    let _ = scratch;
}

/// discard-on-regression: baseline median 10; round 1's runs (20,20,20)
/// regress, so the worktree is reset to `bestRef` and no commit lands, and
/// `git clean -fd` removes the worker's untracked marker file.
#[test]
fn discard_on_regression_resets_and_cleans() {
    let (scratch, repo, home) =
        setup_fixture("p163-discard", "experiment", "optimize-fixture-discard");
    write_queue(
        &repo,
        &home,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":20},{"status":"ok","metric":20},{"status":"ok","metric":20}]"#,
    );
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = run_experiment(&repo, &home, "optimize-fixture-discard", 3, None);
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "iteration-capped experiment always completes:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before, head_after,
        "a regressing candidate must never be committed"
    );
    let status = run_git(&["status", "--porcelain"], &repo, &home);
    let (porcelain, _) = utf8(&status);
    assert!(
        !porcelain.contains("fixture-work-output.txt"),
        "the discard arm's `git clean -fd` must remove the worker's untracked marker: {porcelain:?}"
    );

    // baseline immutability (discard-round case): a reset-to-bestRef round
    // must land HEAD back exactly on the immutable baseline commit.
    assert_baseline_still_ancestor(&repo, &home, &head_before);

    let _ = scratch;
}

/// max-delta-guard-severed regression proof: a candidate that improves the
/// metric but supplies no `delta-lines` is discarded once `max-delta-lines`
/// is set, and a candidate whose `delta-lines` clears the cap is kept with
/// that measured value intact through aggregation.
#[test]
fn max_delta_cap_discards_unmeasured_and_keeps_measured_within_cap() {
    let (scratch, repo, home) =
        setup_fixture("p163-max-delta", "experiment", "optimize-fixture-max-delta");
    write_queue(
        &repo,
        &home,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":5},{"status":"ok","metric":5},{"status":"ok","metric":5}]"#,
    );
    let head_before_unmeasured = git_rev_parse_head(&repo, &home);

    let stdout = run_experiment(&repo, &home, "optimize-fixture-max-delta", 3, Some(50));
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "run must complete:\n{stdout}"
    );
    let head_after_unmeasured = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before_unmeasured, head_after_unmeasured,
        "an improving candidate that never reports delta-lines must be discarded once max-delta-lines is set (supplied-but-unmeasurable therefore discards)"
    );

    // Same run, second round: replace the queue with a delta-lines-bearing
    // candidate that clears the cap. Re-running against the same fixture id
    // would reuse ledger state, so this is asserted as a second, independent
    // fixture instead.
    let (scratch2, repo2, home2) = setup_fixture(
        "p163-max-delta-measured",
        "experiment",
        "optimize-fixture-max-delta-measured",
    );
    write_queue(
        &repo2,
        &home2,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":5,"delta-lines":3},{"status":"ok","metric":5,"delta-lines":3},{"status":"ok","metric":5,"delta-lines":3}]"#,
    );
    let head_before_measured = git_rev_parse_head(&repo2, &home2);
    let stdout2 = run_experiment(
        &repo2,
        &home2,
        "optimize-fixture-max-delta-measured",
        3,
        Some(50),
    );
    assert!(
        stdout2.contains("\"final-state\": \"completed\""),
        "run must complete:\n{stdout2}"
    );
    let head_after_measured = git_rev_parse_head(&repo2, &home2);
    assert_ne!(
        head_before_measured, head_after_measured,
        "an improving candidate reporting delta-lines within the cap must be kept"
    );

    let _ = (scratch, head_after_unmeasured, scratch2);
}

/// iteration-cap park + baseline immutability: `optimize:experiment` never
/// has a target, so every run ends `iteration-limit-reached`; the initial
/// baseline SHA (captured before any round) is asserted unchanged whether
/// the sole round is kept or discarded, since a kept round only ever
/// advances `bestRef`, never the immutable capture the reset arm targets.
#[test]
fn iteration_cap_parks_with_consistent_rows_and_immutable_baseline() {
    let (scratch, repo, home) = setup_fixture("p163-park", "experiment", "optimize-fixture-park");
    write_queue(
        &repo,
        &home,
        r#"[{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":10},{"status":"ok","metric":20},{"status":"ok","metric":20},{"status":"ok","metric":20}]"#,
    );
    let head_before = git_rev_parse_head(&repo, &home);

    let stdout = run_experiment(&repo, &home, "optimize-fixture-park", 3, None);
    assert!(
        stdout.contains("\"final-state\": \"completed\""),
        "run must complete:\n{stdout}"
    );
    assert!(
        stdout.contains("iteration-limit-reached"),
        "the only stop reason for optimize:experiment is iteration-limit-reached:\n{stdout}"
    );
    assert!(
        stdout.contains("\"kept\":0") || stdout.contains("\"kept\": 0"),
        "the sole regressing round must not be counted kept:\n{stdout}"
    );

    let head_after = git_rev_parse_head(&repo, &home);
    assert_eq!(
        head_before, head_after,
        "the sole regressing round must be discarded, leaving HEAD unchanged"
    );
    assert_baseline_still_ancestor(&repo, &home, &head_before);

    let _ = scratch;
}
