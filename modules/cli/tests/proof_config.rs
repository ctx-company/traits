//! Public-path proofs for P418 configuration resolution.

use std::fs;
use std::process::Command;

use support::{
    ScratchRoot, assert_exit_code, git_init, pin_volatile_ledger_fields, repo_root,
    require_success, run_ctx, utf8,
};

/// P476: this must run against a scratch repo, never `repo_root()` — the
/// real repo's own `.ctx/config.toml` is migrated by the owner separately
/// (§7 of the P476 draft: the run must not pre-migrate it, and the old
/// pre-P476 shape parses under this binary only until the owner's own
/// migration lands), so asserting success against it here would couple this
/// public-path proof to that unrelated migration timing.
#[test]
fn doctor_reports_effective_defaults_and_rejects_conflicting_path() {
    let scratch = ScratchRoot::new("p418-doctor");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let stdout = require_success(
        "`ctx traits doctor --config`",
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    assert!(stdout.contains("run.attach-wait-seconds: 1800"), "{stdout}");
    assert!(stdout.contains("run.max-in-flight: 1"), "{stdout}");

    let output = run_ctx(
        &["traits", "doctor", ".", "--config"],
        &repo,
        &scratch.home(),
    );
    assert_ne!(output.status.code(), Some(0));
}

/// Repository requirements are resolved per authored leaf, so an environment
/// document can still supply absent leaves but cannot displace a repository
/// declaration in any requirement family.
#[test]
fn doctor_keeps_repository_requirement_leaves_over_ctx_config() {
    let scratch = ScratchRoot::new("p101-requirement-leaves");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "schema-version = 'repo-schema'\n\
[worktree]\nsetup = []\n\
[run]\nworktree = false\nmax-frames = 7\nstrict-loops = false\n\
[merge]\noverlap = 'park'\ngate = []\n",
    )
    .unwrap();
    let environment = scratch.home().join("environment.toml");
    fs::write(
        &environment,
        "schema-version = 'environment-schema'\n\
[worktree]\nsetup = [['environment-setup']]\n\
[run]\nworktree = true\nmax-frames = 9\nstrict-loops = true\n\
[merge]\noverlap = 'land'\ngate = [['environment-gate']]\n",
    )
    .unwrap();
    let mut command = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    command.env("CTX_CONFIG", &environment);
    let output = command.output().unwrap();
    assert_exit_code(&output, 0);
    let (stdout, _) = utf8(&output);
    for expected in [
        "worktree.setup: 0 [repo:",
        "run.worktree: false [repo:",
        "run.max-frames: 7 [repo:",
        "run.strict-loops: false [repo:",
        "merge.overlap: park [repo:",
        "merge.gate: absent (empty) [repo:",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}: {stdout}");
    }
    assert!(stdout.contains("schema-version rejected from"), "{stdout}");
}

#[test]
fn layered_doctor_reports_exact_leaf_provenance_and_additive_contributors() {
    let scratch = ScratchRoot::new("p1010-layered-provenance");
    let repo = scratch.home().join("repo");
    let global = scratch.home().join("ctx/config.toml");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    fs::create_dir_all(global.parent().unwrap()).unwrap();
    git_init(&repo);

    // Discover the key doctor exposes to users before installing its matching
    // personal override.
    let initial = require_success(
        "discover active repository qualifier",
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    let active_key = initial
        .lines()
        .find(|line| line.trim_start().starts_with("repo.active-key:"))
        .and_then(|line| line.split_once(':'))
        .map(|(_, value)| value.split('[').next().unwrap_or(value).trim())
        .unwrap_or_else(|| panic!("doctor did not report an active key: {initial}"));

    fs::write(
        &global,
        format!(
            "[host.layered]\nformat = 'global-format'\n\
[harness.layered]\nkind = 'custom'\nbin = 'global-bin'\ntransports = ['cli']\nversion-probe = ['--version']\n\
[harness.layered.cli]\nargv = []\nprompt-via = 'stdin'\noutput = 'raw-json'\n\
[harness.layered.mcp]\nallowed-tools = ['global-tool', 'duplicate-tool']\n\
[[agent.role.worker]]\nharness = 'global-worker'\nmodel = 'stale-global-model'\n\
[[agent.role.worker]]\nharness = 'global-second-worker'\n\
[publish]\nexclude = ['global-exclude', 'duplicate-exclude']\n\
[worktree.env]\nGLOBAL_ONLY = 'global'\nCONFLICT = 'global'\n\
[run.build-cache.shared]\nenv = 'GLOBAL_CACHE'\n\
[repo.\"{active_key}\".host.layered]\nprofile = 'personal-profile'\n\
[repo.\"{active_key}\".agent.role.worker]\nmodel = 'personal-model'\n\
[repo.\"{active_key}\".publish]\nexclude = ['personal-exclude', 'duplicate-exclude']\n\
[repo.\"{active_key}\".worktree.env]\nCONFLICT = 'personal'\n\
[repo.\"{active_key}\".run.build-cache.shared]\nenv = 'PERSONAL_CACHE'\n"
        ),
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/config.toml"),
        "[run]\nmax-frames = 7\n\
[host.layered]\nformat = 'repo-format'\n\
[harness.layered]\nbin = 'repo-bin'\n\
[harness.layered.mcp]\nmcp-config-flag = '--repo-mcp'\n\
[[agent.role.worker]]\nharness = 'repo-worker'\n\
[publish]\nexclude = ['repo-exclude', 'duplicate-exclude']\n\
[worktree.env]\nREPO_ONLY = 'repo'\nCONFLICT = 'repo'\n\
[run.build-cache.shared]\nenv = 'REPO_CACHE'\n",
    )
    .unwrap();
    let environment = scratch.home().join("environment.toml");
    fs::write(
        &environment,
        "[host.layered]\nproject-path = 'environment-project'\n\
[[agent.role.worker]]\nharness = 'environment-worker'\n\
[publish]\nexclude = ['environment-exclude', 'duplicate-exclude']\n\
[worktree.env]\nENV_ONLY = 'environment'\nCONFLICT = 'environment'\n\
[run.build-cache.shared]\nenv = 'ENV_CACHE'\n",
    )
    .unwrap();
    let global_source = fs::canonicalize(&global).unwrap().display().to_string();
    let repo_source = fs::canonicalize(&repo)
        .unwrap()
        .join(".")
        .join(".ctx/config.toml")
        .display()
        .to_string();
    let environment_source = environment.display().to_string();

    let mut text_command = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    text_command.env("CTX_CONFIG", &environment);
    let text_output = text_command.output().unwrap();
    assert_exit_code(&text_output, 0);
    let (text, _) = utf8(&text_output);
    for expected in [
        format!("host.layered.profile: personal-profile [user-global: {global_source}] [your per-repo override]"),
        format!("host.layered.format: repo-format [repo: {repo_source}] [repo default]"),
        format!("host.layered.project-path: environment-project [environment: {environment_source}] [environment override]"),
        format!("harness.layered.cli.prompt-via: stdin [user-global: {global_source}] [default]"),
        format!("harness.layered.bin: repo-bin [repo: {repo_source}] [repo default]"),
        format!("harness.layered.mcp.mcp-config-flag: --repo-mcp [repo: {repo_source}] [repo default]"),
        format!("agent.role.worker.1.harness: environment-worker [environment: {environment_source}] [environment override]"),
        format!("run.max-frames: 7 [repo: {repo_source}] [repo requirement]"),
        "publish.exclude: [\".git\",\"node_modules\",\"target\",\".turbo\",\"global-exclude\",\"duplicate-exclude\",\"repo-exclude\",\"environment-exclude\",\"personal-exclude\"]".to_string(),
        format!("worktree.env.CONFLICT: repo [repo: {repo_source}] [additive]"),
        format!("run.build-cache.shared.env: REPO_CACHE [repo: {repo_source}] [additive]"),
    ] {
        assert!(text.contains(&expected), "missing {expected:?}: {text}");
    }

    let mut json_command = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config", "--json"],
        &repo,
        &scratch.home(),
    );
    json_command.env("CTX_CONFIG", &environment);
    let json_output = json_command.output().unwrap();
    assert_exit_code(&json_output, 0);
    let (json, _) = utf8(&json_output);
    let report: serde_json::Value = serde_json::from_str(&json).unwrap_or_else(|error| {
        panic!("doctor --config --json did not emit JSON: {error}\n{json}")
    });
    let knobs = &report["knobs"];
    for (key, value, layer, source, winner_reason, reason) in [
        (
            "host.layered.profile",
            "personal-profile",
            "user-global",
            &global_source,
            "personal-repo-override",
            "your per-repo override",
        ),
        (
            "host.layered.format",
            "repo-format",
            "repo",
            &repo_source,
            "repo-default",
            "repo default",
        ),
        (
            "host.layered.project-path",
            "environment-project",
            "environment",
            &environment_source,
            "environment-override",
            "environment override",
        ),
        (
            "harness.layered.cli.prompt-via",
            "stdin",
            "user-global",
            &global_source,
            "default",
            "default",
        ),
        (
            "harness.layered.mcp.mcp-config-flag",
            "--repo-mcp",
            "repo",
            &repo_source,
            "repo-default",
            "repo default",
        ),
        (
            "run.max-frames",
            "7",
            "repo",
            &repo_source,
            "repo-requirement",
            "repo requirement",
        ),
        (
            "agent.role.worker.1.harness",
            "environment-worker",
            "environment",
            &environment_source,
            "environment-override",
            "environment override",
        ),
    ] {
        assert_eq!(knobs[key]["value"], value, "{key}: {report}");
        assert_eq!(knobs[key]["winner"]["layer"], layer, "{key}: {report}");
        assert_eq!(
            knobs[key]["winner"]["source"],
            source.as_str(),
            "{key}: {report}"
        );
        assert_eq!(
            knobs[key]["winner"]["reason"], winner_reason,
            "{key}: {report}"
        );
        assert_eq!(knobs[key]["reason"], reason, "{key}: {report}");
    }
    assert_eq!(knobs["worktree.env.CONFLICT"]["value"], "repo");
    assert_eq!(knobs["worktree.env.CONFLICT"]["winner"]["layer"], "repo");
    assert_eq!(
        knobs["worktree.env.CONFLICT"]["winner"]["source"],
        repo_source
    );
    assert_eq!(
        knobs["worktree.env.CONFLICT"]["winner"]["reason"],
        "additive"
    );
    assert_eq!(
        knobs["agent.role.worker.1.model"]["winner"]["layer"], "built-in",
        "replaced list seat retained a stale global model winner: {report}"
    );
    assert_eq!(
        knobs["harness.layered.mcp.allowed-tools"]["winner"]["layer"], "built-in",
        "the global MCP table was replaced rather than retaining its stale winner"
    );
    assert_eq!(knobs["run.build-cache.shared.env"]["value"], "REPO_CACHE");
    assert_eq!(
        knobs["run.build-cache.shared.env"]["winner"]["layer"],
        "repo"
    );
    assert_eq!(
        knobs["run.build-cache.shared.env"]["winner"]["source"],
        repo_source
    );
    assert_eq!(
        knobs["run.build-cache.shared.env"]["winner"]["reason"],
        "additive"
    );
    let contributors = knobs["publish.exclude"]["winner"]["contributors"]
        .as_array()
        .unwrap_or_else(|| panic!("missing publish contributors: {report}"));
    assert_eq!(
        contributors,
        &vec![
            serde_json::json!({ "layer": "built-in", "source": serde_json::Value::Null }),
            serde_json::json!({ "layer": "user-global", "source": global_source }),
            serde_json::json!({ "layer": "repo", "source": repo_source }),
            serde_json::json!({ "layer": "environment", "source": environment_source }),
        ]
    );
    assert_eq!(
        knobs["worktree.env.CONFLICT"]["winner"]["contributors"],
        serde_json::json!([
            { "layer": "user-global", "source": global_source },
            { "layer": "repo", "source": repo_source },
            { "layer": "environment", "source": environment_source },
        ]),
        "the global root and personal block must share one contributor"
    );
    assert_eq!(
        knobs["run.build-cache.shared.env"]["winner"]["contributors"],
        serde_json::json!([
            { "layer": "user-global", "source": global_source },
            { "layer": "repo", "source": repo_source },
            { "layer": "environment", "source": environment_source },
        ])
    );
}

/// P476: a pre-P476 `[agent.master]`/`[agent.narrator]`/`[agent.merger]`/
/// `[agent.merger-deep]` table, or a pre-P476 bare `[agent]` scalar, fails
/// decode with a message naming its exact `[agent.role.*]` replacement —
/// never serde's generic "unknown field" text.
#[test]
fn legacy_agent_keys_fail_parse_naming_their_role_replacement() {
    let scratch = ScratchRoot::new("p476-legacy-agent-keys");
    for (legacy, hint) in [
        (
            "[agent.master]\nharness = \"claude\"\n",
            "[agent.role.default]",
        ),
        (
            "[agent.narrator]\nharness = \"claude\"\n",
            "[agent.role.narrator]",
        ),
        (
            "[agent.merger]\nharness = \"claude\"\n",
            "[agent.role.merger]",
        ),
        (
            "[agent.merger-deep]\nharness = \"claude\"\n",
            "[agent.role.merger-deep]",
        ),
        (
            "[agent]\nmodel = \"sonnet\"\n",
            "[agent.role.default].model",
        ),
        (
            "[agent]\nharness = \"claude\"\n",
            "[agent.role.default].harness",
        ),
    ] {
        let config = scratch.home().join("repo.toml");
        fs::write(&config, legacy).unwrap();
        let mut command = support::controlled_command(
            &support::ctx_bin(),
            &["traits", "doctor", "--config"],
            &repo_root(),
            &scratch.home(),
        );
        command.env("CTX_CONFIG", &config);
        let output = command.output().unwrap();
        assert_ne!(output.status.code(), Some(0), "config: {legacy:?}");
        let (_, stderr) = utf8(&output);
        assert!(
            stderr.contains(hint),
            "expected {hint:?} in error for {legacy:?}, got: {stderr}"
        );
    }
}

/// P514: `ctx traits doctor --migrate-config` reports the exact
/// `[agent.role.*]` rewrite for every pre-P476 legacy `[agent]` key across a
/// config carrying all four legacy tables and a bare scalar, interleaved
/// with comments and unrelated `[harness.*]`/`[merge]` tables; `--apply`
/// performs the non-conflicting rewrites, preserving comments and unrelated
/// tables byte-for-byte, and produces a file `doctor --config` then decodes
/// at exit 0. A second `--migrate-config` on the now-clean file reports an
/// empty plan. Separately, a pre-existing `[agent.role.default]` alongside
/// `[agent.master]` is reported as a conflict and left untouched.
#[test]
fn migrate_config_rewrites_legacy_agent_keys_and_reports_conflicts() {
    let scratch = ScratchRoot::new("p514-migrate-config");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    let config_path = repo.join(".ctx/config.toml");
    fs::write(
        &config_path,
        "# operator notes: seats tuned 2026-07\n\
[agent]\n\
model = \"sonnet\"\n\
\n\
# the driver seat\n\
[agent.master]\n\
harness = \"claude\"\n\
\n\
[agent.narrator]\n\
harness = \"opencode\"\n\
\n\
[agent.merger]\n\
harness = \"claude\"\n\
model = \"sonnet\"\n\
reasoning-effort = \"medium\"\n\
\n\
[agent.merger-deep]\n\
harness = \"claude\"\n\
model = \"opus\"\n\
reasoning-effort = \"high\"\n\
\n\
[publish]\n\
exclude = [\"scratch\"]\n\
\n\
[merge]\n\
deep = true\n",
    )
    .unwrap();

    // (a) fails decode today.
    let decode_output = run_ctx(&["traits", "doctor", "--config"], &repo, &scratch.home());
    assert_ne!(decode_output.status.code(), Some(0));

    // (b) `--migrate-config` reports every rewrite in one pass.
    let plan_stdout = require_success(
        "`ctx traits doctor --migrate-config`",
        &["traits", "doctor", "--migrate-config"],
        &repo,
        &scratch.home(),
    );
    for expect in [
        "agent.master -> agent.role.default",
        "agent.narrator -> agent.role.narrator",
        "agent.merger -> agent.role.merger",
        "agent.merger-deep -> agent.role.merger-deep",
        "agent.model -> agent.role.default.model",
    ] {
        assert!(
            plan_stdout.contains(expect),
            "expected {expect:?} in plan: {plan_stdout}"
        );
    }

    // (c) `--apply` produces a file `doctor --config` then decodes at exit 0.
    require_success(
        "`ctx traits doctor --migrate-config --apply`",
        &["traits", "doctor", "--migrate-config", "--apply"],
        &repo,
        &scratch.home(),
    );
    let config_stdout = require_success(
        "`ctx traits doctor --config` after migration",
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    assert!(
        config_stdout.contains("agent.role.default"),
        "{config_stdout}"
    );
    assert!(
        config_stdout.contains("agent.role.narrator"),
        "{config_stdout}"
    );
    assert!(
        config_stdout.contains("agent.role.merger"),
        "{config_stdout}"
    );
    assert!(
        config_stdout.contains("agent.role.merger-deep"),
        "{config_stdout}"
    );

    // (d) comments and unrelated tables survive byte-for-byte.
    let migrated = fs::read_to_string(&config_path).unwrap();
    assert!(
        migrated.contains("# operator notes: seats tuned 2026-07"),
        "{migrated}"
    );
    assert!(migrated.contains("# the driver seat"), "{migrated}");
    assert!(
        migrated.contains("[publish]\nexclude = [\"scratch\"]\n"),
        "{migrated}"
    );
    assert!(migrated.contains("[merge]\ndeep = true\n"), "{migrated}");
    assert!(!migrated.contains("[agent.master]"), "{migrated}");
    assert!(!migrated.contains("[agent.narrator]"), "{migrated}");

    // (e) a second `--migrate-config` on the now-clean file reports an empty plan.
    let second_plan = require_success(
        "second `ctx traits doctor --migrate-config`",
        &["traits", "doctor", "--migrate-config"],
        &repo,
        &scratch.home(),
    );
    assert!(
        second_plan.contains("no legacy [agent] keys found"),
        "{second_plan}"
    );

    // (f) a pre-existing [agent.role.default] alongside [agent.master] is a
    // reported conflict, left untouched.
    let conflict_repo = scratch.home().join("conflict-repo");
    fs::create_dir_all(conflict_repo.join(".ctx")).unwrap();
    git_init(&conflict_repo);
    let conflict_config = conflict_repo.join(".ctx/config.toml");
    let conflict_text =
        "[agent.role.default]\nharness = \"claude\"\n\n[agent.master]\nharness = \"claude\"\n";
    fs::write(&conflict_config, conflict_text).unwrap();
    let conflict_plan = require_success(
        "`ctx traits doctor --migrate-config` on a conflicting config",
        &["traits", "doctor", "--migrate-config"],
        &conflict_repo,
        &scratch.home(),
    );
    assert!(
        conflict_plan.contains("conflict: agent.master -> agent.role.default"),
        "{conflict_plan}"
    );
    require_success(
        "`ctx traits doctor --migrate-config --apply` on a conflicting config",
        &["traits", "doctor", "--migrate-config", "--apply"],
        &conflict_repo,
        &scratch.home(),
    );
    let conflict_after = fs::read_to_string(&conflict_config).unwrap();
    assert_eq!(
        conflict_after, conflict_text,
        "conflict entry must be left untouched"
    );
}

/// P514 review fix: an inline-table `[agent]` (`agent = { master = { ... } }`)
/// cannot hold a standard `[agent.role.*]` table inserted under it — the
/// rendered text silently drops the insertion even though the in-memory
/// `toml_edit` tree looks correct. `--migrate-config` must never report or
/// write a rewrite it cannot actually render: `--apply` on such a layer must
/// leave the file byte-identical and report no successful rewrite for it.
#[test]
fn migrate_config_rewrites_an_inline_agent_table_losslessly() {
    let scratch = ScratchRoot::new("p514-migrate-config-inline-agent");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    let config_path = repo.join(".ctx/config.toml");
    let original_text = "agent = { master = { harness = \"opencode\", model = \"gpt\" } }\n";
    fs::write(&config_path, original_text).unwrap();

    let plan_stdout = require_success(
        "`ctx traits doctor --migrate-config` on an inline [agent] table",
        &["traits", "doctor", "--migrate-config"],
        &repo,
        &scratch.home(),
    );
    // P569: an inline `agent = { master = ... }` table now DOES survive the
    // round trip — the rewrite renders `agent = { role.default = ... }`, which
    // is valid TOML, resolves to the same `agent.role.default`, and loses
    // nothing. The guard this test protects is still live (a rewrite that
    // fails verification is still refused); this particular input simply
    // stopped being an example of one, so asserting a refusal here would pin
    // the tool to a limitation it no longer has.
    assert!(
        plan_stdout.contains("agent.master -> agent.role.default"),
        "an inline [agent] table that round-trips must be reported as rewritable: {plan_stdout}"
    );

    require_success(
        "`ctx traits doctor --migrate-config --apply` on an inline [agent] table",
        &["traits", "doctor", "--migrate-config", "--apply"],
        &repo,
        &scratch.home(),
    );
    let after = fs::read_to_string(&config_path).unwrap();
    assert_eq!(
        after, "agent = { role.default = { harness = \"opencode\", model = \"gpt\" } }\n",
        "a verified rewrite must land the [agent.role.*] destination losslessly"
    );
    assert!(
        !after.contains("master"),
        "the legacy key must be gone after a rewrite, not left beside its replacement: {after}"
    );
}

/// P476: every seat — the `default` driver, `narrator`, `merger`,
/// `merger-deep`, and a declared trait role — renders each authored assignment
/// leaf with its own config layer/source rather than attaching one broad role
/// winner to fields that may inherit from different layers.
#[test]
fn doctor_config_renders_every_seat_through_one_uniform_listing() {
    let scratch = ScratchRoot::new("p476-doctor-uniform-seats");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "[agent.role.default]\nharness = 'claude'\n\n\
[agent.role.narrator]\nharness = 'claude'\n\n\
[agent.role.merger]\nharness = 'claude'\nmodel = 'sonnet'\nreasoning-effort = 'medium'\n\n\
[agent.role.merger-deep]\nharness = 'claude'\nmodel = 'opus'\nreasoning-effort = 'high'\n\n\
[agent.role.worker]\nharness = 'claude'\n",
    )
    .unwrap();
    let output = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    )
    .output()
    .unwrap();
    assert_exit_code(&output, 0);
    let (stdout, _) = utf8(&output);
    for role in ["default", "narrator", "merger", "merger-deep", "worker"] {
        let line = stdout
            .lines()
            .find(|line| {
                line.trim_start()
                    .starts_with(&format!("agent.role.{role}.harness: "))
            })
            .unwrap_or_else(|| panic!("missing agent.role.{role}.harness listing: {stdout}"));
        assert!(
            line.contains(": claude "),
            "agent.role.{role}.harness must report its configured harness: {line}"
        );
        assert!(
            line.contains("[repo:"),
            "agent.role.{role}.harness must carry its repo-layer source: {line}"
        );
    }
}

#[test]
fn config_max_in_flight_is_validated_on_public_path() {
    let scratch = ScratchRoot::new("p418-validation");
    let config = scratch.home().join("repo.toml");
    fs::write(&config, "[run]\nmax-in-flight = 0\n").unwrap();
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
    assert!(stderr.contains("run.max-in-flight"), "{stderr}");
}

#[test]
fn doctor_uses_repo_defaults_after_global_defaults() {
    let scratch = ScratchRoot::new("p418-precedence");
    let repo = scratch.home().join("repo");
    let global = scratch.home().join("ctx");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    fs::create_dir_all(&global).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "[run]\nmax-in-flight = 2\n[agent.role.worker]\nharness = 'repo-worker'\n",
    )
    .unwrap();
    fs::write(
        global.join("config.toml"),
        "[agent.role.worker]\nharness = 'global-worker'\n",
    )
    .unwrap();
    let output = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    )
    .output()
    .unwrap();
    assert_exit_code(&output, 0);
    let (stdout, _) = utf8(&output);
    assert!(stdout.contains("run.max-in-flight: 2 [repo:"), "{stdout}");
    assert!(
        stdout.contains("agent.role.worker.harness: repo-worker"),
        "{stdout}"
    );
    assert!(stdout.contains("[repo default]"), "{stdout}");
}

/// P477: with no `[merge] gate` declared anywhere, doctor reports the
/// product-default empty gate and the default per-command ceiling, both
/// attributed to the built-in layer.
#[test]
fn merge_gate_defaults_to_empty_and_default_ceiling() {
    let scratch = ScratchRoot::new("p477-gate-defaults");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let output = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    )
    .output()
    .unwrap();
    assert_exit_code(&output, 0);
    let (stdout, _) = utf8(&output);
    assert!(
        stdout.contains("merge.gate: absent (empty) [built-in]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("merge.gate-seconds: 1800 [built-in]"),
        "{stdout}"
    );
}

/// P477: a nearer (repo) layer's explicit `gate = []` must clear a farther
/// (user-global) layer's declared gate wholesale — never merge/append the
/// two — and doctor must attribute the winning (repo) source, not the
/// farther global one.
#[test]
fn merge_gate_explicit_empty_overrides_a_farther_declaration() {
    let scratch = ScratchRoot::new("p477-gate-override");
    let repo = scratch.home().join("repo");
    let global = scratch.home().join("ctx");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    fs::create_dir_all(&global).unwrap();
    git_init(&repo);
    fs::write(repo.join(".ctx/config.toml"), "[merge]\ngate = []\n").unwrap();
    fs::write(
        global.join("config.toml"),
        "[merge]\ngate = [[\"global-only-cmd\"]]\n",
    )
    .unwrap();
    let output = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    )
    .output()
    .unwrap();
    assert_exit_code(&output, 0);
    let (stdout, _) = utf8(&output);
    assert!(
        stdout.contains("merge.gate: absent (empty) [repo:"),
        "an explicit empty repo-layer gate must win over the farther global declaration: {stdout}"
    );
}

/// P477: a nearer layer's declared gate replaces a farther one wholesale
/// (never concatenates), and doctor attributes the winning layer.
#[test]
fn merge_gate_repo_declaration_replaces_global_wholesale() {
    let scratch = ScratchRoot::new("p477-gate-replace");
    let repo = scratch.home().join("repo");
    let global = scratch.home().join("ctx");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    fs::create_dir_all(&global).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "[merge]\ngate = [[\"repo-cmd\"]]\ngate-seconds = 30\n",
    )
    .unwrap();
    fs::write(
        global.join("config.toml"),
        "[merge]\ngate = [[\"global-cmd\"], [\"another-global-cmd\"]]\n",
    )
    .unwrap();
    let output = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    )
    .output()
    .unwrap();
    assert_exit_code(&output, 0);
    let (stdout, _) = utf8(&output);
    assert!(
        stdout.contains(r#"merge.gate: [["repo-cmd"]] [repo:"#),
        "the nearer repo declaration must replace the farther one wholesale, never concatenate: {stdout}"
    );
    assert!(stdout.contains("merge.gate-seconds: 30 [repo:"), "{stdout}");
}

/// P477: an invalid `[merge] gate-seconds` (zero) must fail validation on
/// the public path, exactly like `run.max-in-flight`'s existing validation.
/// Runs against a SCRATCH repo, never `repo_root()` — same reason as the
/// P476 note above: this repository now declares its own `[merge] gate`
/// (0056's landing gate), and a repo-owned requirement leaf legitimately
/// overrides a personal `$CTX_CONFIG` declaration, so pointing this proof at
/// the live repo would test precedence rather than the validation it exists
/// to pin.
#[test]
fn merge_gate_seconds_zero_is_rejected_on_public_path() {
    let scratch = ScratchRoot::new("p477-gate-seconds-validation");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let config = scratch.home().join("repo.toml");
    fs::write(&config, "[merge]\ngate-seconds = 0\n").unwrap();
    let mut command = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    command.env("CTX_CONFIG", &config);
    let output = command.output().unwrap();
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(stderr.contains("merge.gate-seconds"), "{stderr}");
}

/// P477: a declared gate command with no executable (an empty argv) must
/// fail validation on the public path.
/// Runs against a SCRATCH repo, never `repo_root()` — same reason as the
/// P476 note above: this repository now declares its own `[merge] gate`
/// (0056's landing gate), and a repo-owned requirement leaf legitimately
/// overrides a personal `$CTX_CONFIG` declaration, so pointing this proof at
/// the live repo would test precedence rather than the validation it exists
/// to pin.
#[test]
fn merge_gate_empty_command_is_rejected_on_public_path() {
    let scratch = ScratchRoot::new("p477-gate-empty-command");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let config = scratch.home().join("repo.toml");
    fs::write(&config, "[merge]\ngate = [[]]\n").unwrap();
    let mut command = support::controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    command.env("CTX_CONFIG", &config);
    let output = command.output().unwrap();
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(stderr.contains("merge.gate"), "{stderr}");
}

// ---------------------------------------------------------------------------
// P475: per-role seat budgets
// ---------------------------------------------------------------------------

/// P475: with no budget declared anywhere, every seat's `doctor --config`
/// row reports today's built-in numbers (300s frame budget, 1 retry for
/// frame-dispatched seats, 900s for `merger`/`merger-deep`, 20s for
/// `narrator`), and the derived lock-wait ceiling equals the product those
/// defaults have always produced — the "byte-identical to today" claim,
/// asserted as values rather than a captured blob.
#[test]
fn seat_budgets_default_to_todays_built_in_numbers() {
    let scratch = ScratchRoot::new("p475-budget-defaults");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let stdout = require_success(
        "`ctx traits doctor --config`",
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    assert!(
        stdout.contains("agent.role.default.budget.frame-seconds: 300 [built-in]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("agent.role.default.budget.max-retries: 1 [built-in]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("agent.role.merger.budget.frame-seconds: 900 [built-in]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("agent.role.merger-deep.budget.frame-seconds: 900 [built-in]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("agent.role.narrator.budget.frame-seconds: 20 [built-in]"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "merge.lock-wait-derived-seconds.standard: 22560 (= 25 * merger-budget[900s] + gate[0s] + overhead[60s]) [built-in]"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "merge.lock-wait-derived-seconds.deep: 22560 (= 25 * merger-deep-budget[900s] + gate[0s] + overhead[60s]) [built-in]"
        ),
        "{stdout}"
    );
}

/// P475: a seat's own declared `[agent.role.<name>.budget]` renders with the
/// resolved value and the declaring layer's source, and a distinct value on
/// a different seat resolves independently — no cross-seat bleed.
#[test]
fn declared_seat_budget_renders_value_and_source_layer() {
    let scratch = ScratchRoot::new("p475-budget-provenance");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "[agent.role.merger-deep]\nharness = 'claude'\nmodel = 'opus'\nreasoning-effort = 'high'\n\n\
[agent.role.merger-deep.budget]\nframe-seconds = 1200\n\n\
[agent.role.narrator]\nharness = 'claude'\n\n\
[agent.role.narrator.budget]\nframe-seconds = 45\n",
    )
    .unwrap();
    let stdout = require_success(
        "`ctx traits doctor --config`",
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    let merger_deep_line = stdout
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("agent.role.merger-deep.budget.frame-seconds:")
        })
        .unwrap_or_else(|| panic!("missing merger-deep budget row: {stdout}"));
    assert!(
        merger_deep_line.contains("1200") && merger_deep_line.contains("[repo:"),
        "{merger_deep_line}"
    );
    let narrator_line = stdout
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("agent.role.narrator.budget.frame-seconds:")
        })
        .unwrap_or_else(|| panic!("missing narrator budget row: {stdout}"));
    assert!(
        narrator_line.contains("45") && narrator_line.contains("[repo:"),
        "{narrator_line}"
    );
    // merger (undeclared) must stay at its own built-in default, unaffected
    // by merger-deep's declared value.
    assert!(
        stdout.contains("agent.role.merger.budget.frame-seconds: 900 [built-in]"),
        "{stdout}"
    );
}

/// P475 D5: raising the merger seat's declared budget moves the derived
/// lock-wait ceiling by exactly 25x the delta (`MAX_RECONCILIATION_ITERATIONS`),
/// surfaced BEFORE a batch would wedge on the resulting worst-case wait.
#[test]
fn raising_merger_budget_moves_derived_lock_wait_by_25x_delta() {
    let scratch = ScratchRoot::new("p475-lock-wait-derivation");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "[agent.role.merger]\nharness = 'claude'\nmodel = 'sonnet'\nreasoning-effort = 'medium'\n\n\
[agent.role.merger.budget]\nframe-seconds = 1000\n",
    )
    .unwrap();
    let stdout = require_success(
        "`ctx traits doctor --config`",
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    // 900s -> 1000s is a +100s delta, times 25 = +2500s over the 22560s
    // built-in-default derivation asserted in
    // `seat_budgets_default_to_todays_built_in_numbers`. Both rows move: no
    // `merger-deep` table is declared, so the `.deep` row falls back to the
    // same `merger` budget `resolve_merger`/`resolved_merger_role_budget`
    // fall back to for a real `--deep` merge with no deep-specific seat.
    assert!(
        stdout.contains("merge.lock-wait-derived-seconds.standard: 25060"),
        "{stdout}"
    );
    assert!(
        stdout.contains("merge.lock-wait-derived-seconds.deep: 25060"),
        "{stdout}"
    );
}

/// P475 D7 regression: a list-backed role's (`[[agent.role.<name>]]`, P456)
/// per-seat budget rows must pair each seat's LABEL (the 1-based
/// `seat_index` the row key already uses) with THAT SAME seat's resolved
/// budget — never a seat rotated by one, which a base mismatch between the
/// label and `budget_for_seat`'s 0-based `structural_seat` parameter would
/// produce.
#[test]
fn list_backed_role_budget_rows_pair_the_correct_seat() {
    let scratch = ScratchRoot::new("p475-seat-budget-pairing");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "[[agent.role.worker]]\nharness = 'claude'\n\n\
[agent.role.worker.budget]\nframe-seconds = 111\n\n\
[[agent.role.worker]]\nharness = 'claude'\n\n\
[agent.role.worker.budget]\nframe-seconds = 222\n",
    )
    .unwrap();
    let stdout = require_success(
        "`ctx traits doctor --config`",
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    assert!(
        stdout.contains("agent.role.worker.1.budget.frame-seconds: 111"),
        "seat 1 must report its OWN declared budget (111), not seat 2's: {stdout}"
    );
    assert!(
        stdout.contains("agent.role.worker.2.budget.frame-seconds: 222"),
        "seat 2 must report its OWN declared budget (222), not seat 1's: {stdout}"
    );
}

/// P475 §3 D2: a run-level `[run] frame-seconds` masks a declared role
/// budget for drive frames — the documented precedence (CLI > `[run]` >
/// sidecar > role budget > built-in), proved rather than only asserted in a
/// comment. `doctor --config` shows the seat's OWN budget (frame-dispatch
/// precedence only applies inside the drive loop, not to this listing),
/// while `run.frame-seconds` shows the masking value.
#[test]
fn run_level_frame_seconds_is_visible_alongside_a_masked_role_budget() {
    let scratch = ScratchRoot::new("p475-run-level-masks-role-budget");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "[run]\nframe-seconds = 42\n\n\
[agent.role.worker]\nharness = 'claude'\n\n\
[agent.role.worker.budget]\nframe-seconds = 777\n",
    )
    .unwrap();
    let stdout = require_success(
        "`ctx traits doctor --config`",
        &["traits", "doctor", "--config"],
        &repo,
        &scratch.home(),
    );
    assert!(stdout.contains("run.frame-seconds: 42 [repo:"), "{stdout}");
    assert!(
        stdout.contains("agent.role.worker.budget.frame-seconds: 777 [repo:"),
        "a role budget must still be visible in doctor's per-seat listing even though `[run]` would mask it for drive frames: {stdout}"
    );
}

/// P475 D4: `narrator`/`merger`/`merger-deep` are one-shot dispatches with
/// no idle timeout and no retry loop, so a declared `idle-seconds` or
/// `max-retries` on one of those three seats must fail decode with the
/// exact field path — accepting-and-ignoring would let an operator believe
/// a limit is in force when it never takes effect.
#[test]
fn one_shot_seat_budget_rejects_idle_seconds_and_max_retries() {
    let scratch = ScratchRoot::new("p475-one-shot-budget-rejected");
    for (config, expected_path) in [
        (
            "[agent.role.narrator]\nharness = 'claude'\n\n[agent.role.narrator.budget]\nidle-seconds = 5\n",
            "agent.role.narrator.budget.idle-seconds",
        ),
        (
            "[agent.role.merger]\nharness = 'claude'\nmodel = 'sonnet'\nreasoning-effort = 'medium'\n\n\
[agent.role.merger.budget]\nmax-retries = 2\n",
            "agent.role.merger.budget.max-retries",
        ),
    ] {
        let repo = scratch.home().join(expected_path.replace('.', "-"));
        fs::create_dir_all(repo.join(".ctx")).unwrap();
        git_init(&repo);
        fs::write(repo.join(".ctx/config.toml"), config).unwrap();
        let output = run_ctx(&["traits", "doctor", "--config"], &repo, &scratch.home());
        assert_ne!(output.status.code(), Some(0), "config: {config:?}");
        let (_, stderr) = utf8(&output);
        assert!(
            stderr.contains(expected_path),
            "expected {expected_path:?} in error for {config:?}, got: {stderr}"
        );
    }
}

/// P475 §3 D1: a role budget table naming a drive-scoped key (`max-frames`,
/// `total-seconds`, `attach-wait-seconds`) is a hard decode error, never a
/// silently-ignored key — `RoleBudget`'s `deny_unknown_fields`.
#[test]
fn role_budget_table_rejects_drive_scoped_keys() {
    let scratch = ScratchRoot::new("p475-budget-deny-unknown-fields");
    let config = scratch.home().join("repo.toml");
    fs::write(
        &config,
        "[agent.role.worker]\nharness = 'claude'\n\n[agent.role.worker.budget]\nmax-frames = 10\n",
    )
    .unwrap();
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
    // The toml crate does not always surface the offending field name for a
    // nested-table `deny_unknown_fields` violation, so this asserts the
    // decode-error CLASS (never a silently-ignored key) rather than the
    // exact field-name text.
    assert!(stderr.contains("toml decode error"), "{stderr}");
}

/// P475: `frame-seconds = 0` / `idle-seconds = 0` in a role budget table are
/// rejected on the public path, mirroring `merge.gate-seconds`'s existing
/// zero-rejection precedent.
#[test]
fn role_budget_zero_seconds_is_rejected_on_public_path() {
    let scratch = ScratchRoot::new("p475-budget-zero-seconds");
    let config = scratch.home().join("repo.toml");
    fs::write(
        &config,
        "[agent.role.worker]\nharness = 'claude'\n\n[agent.role.worker.budget]\nframe-seconds = 0\n",
    )
    .unwrap();
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
        stderr.contains("agent.role.worker.budget.frame-seconds"),
        "{stderr}"
    );
}

#[test]
fn run_and_session_start_reject_contradictory_policy_flags() {
    let scratch = ScratchRoot::new("p418-flag-conflicts");
    for args in [
        &["traits", "run", "--worktree", "--no-worktree", "--no-drive"][..],
        &[
            "traits",
            "run",
            "--strict-loops",
            "--no-strict-loops",
            "--no-drive",
        ][..],
        &["traits", "session", "start", "--worktree", "--no-worktree"][..],
        &[
            "traits",
            "session",
            "start",
            "--strict-loops",
            "--no-strict-loops",
        ][..],
    ] {
        let output = run_ctx(args, &repo_root(), &scratch.home());
        assert_ne!(
            output.status.code(),
            Some(0),
            "accepted conflicting args: {args:?}"
        );
        let (_, stderr) = utf8(&output);
        assert!(stderr.contains("cannot be used with"), "{stderr}");
    }
}

#[test]
fn flagged_and_configured_drives_preserve_identical_ledger_bytes() {
    let scratch = ScratchRoot::new("p418-ledger-equivalence");
    let repo = scratch.home().join("repo");
    let fixture = repo.join(".ctx/traits/p418-command-only/generated/index.toml");
    let seed = scratch.home().join("seed.json");
    let flagged = scratch.home().join("flagged.json");
    let configured = scratch.home().join("configured.json");

    fs::create_dir_all(fixture.parent().unwrap()).unwrap();
    git_init(&repo);
    fs::write(
        &fixture,
        r#"id = "p418-command-only"
schema-version = "0.2"
version = "0.1.0"
name = "P418 command-only proof"
summary = "A provider-free pending ledger for policy equivalence."

[procedure]
description = "Run one deterministic command."

[[slot]]
id = "notified"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Run command"
kind = "command"
cmd = "true"
output = ["slot:notified"]
"#,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/p418-command-only/trait.toml"),
        "[package]\nid = \"p418-command-only\"\nversion = \"0.1.0\"\nname = \"P418 command-only proof\"\nstatus = \"draft\"\n",
    )
    .unwrap();

    assert_exit_code(
        &run_ctx(
            &[
                "traits",
                "review",
                "--file",
                fixture.to_str().unwrap(),
                "--approve",
            ],
            &repo,
            &scratch.home(),
        ),
        0,
    );
    assert_exit_code(
        &run_ctx(
            &["traits", "activate", "--file", fixture.to_str().unwrap()],
            &repo,
            &scratch.home(),
        ),
        0,
    );

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            fixture.to_str().unwrap(),
            "--no-drive",
            "--out",
            seed.to_str().unwrap(),
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);
    fs::copy(&seed, &flagged).unwrap();
    fs::copy(&seed, &configured).unwrap();

    fs::write(
        repo.join(".ctx/config.toml"),
        "[run]\nworktree = false\nmax-frames = 10\nframe-seconds = 20\ntotal-seconds = 30\nmax-retries = 2\nattach-wait-seconds = 4\nidle-seconds = 5\nmax-in-flight = 1\nwait = false\nstrict-loops = false\n",
    )
    .unwrap();

    let flagged_output = run_ctx(
        &[
            "traits",
            "drive",
            "--file",
            fixture.to_str().unwrap(),
            "--session",
            flagged.to_str().unwrap(),
            "--max-frames",
            "10",
            "--frame-seconds",
            "20",
            "--total-seconds",
            "30",
            "--max-retries",
            "2",
            "--attach-wait-seconds",
            "4",
            "--idle-seconds",
            "5",
            "--max-in-flight",
            "1",
            "--no-wait",
            "--no-worktree",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&flagged_output, 0);

    let configured_output = run_ctx(
        &[
            "traits",
            "drive",
            "--file",
            fixture.to_str().unwrap(),
            "--session",
            configured.to_str().unwrap(),
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&configured_output, 0);

    // `recorded-at-epoch` is wall-clock seconds stamped at drive completion:
    // two back-to-back sub-second drives straddle a second boundary roughly
    // one run in six, which flaked this assertion through four pre-landing
    // gates on 2026-07-24 before the diff was captured (exactly one field,
    // off by one) — and again on 2026-07-25, consistent with a second
    // wall-clock field crossing a boundary the single-field pin missed.
    // Equivalence is about everything the CONFIG produced, so every
    // wall-clock-carrying field name (P461's standing FLAKE-ledger fix,
    // `support::pin_volatile_ledger_fields`) is pinned to a constant on both
    // sides and every other byte still compares exactly.
    let flagged_text =
        String::from_utf8(fs::read(&flagged).unwrap()).expect("ledger is UTF-8 JSON");
    let configured_text =
        String::from_utf8(fs::read(&configured).unwrap()).expect("ledger is UTF-8 JSON");
    assert_eq!(
        pin_volatile_ledger_fields(&flagged_text),
        pin_volatile_ledger_fields(&configured_text),
        "fully flagged and config-only drives must persist identical ledger bytes \
         (wall-clock-carrying fields pinned on both sides)"
    );
}

// ---------------------------------------------------------------------------
// P428: named build-cache declarations and their prune path
// ---------------------------------------------------------------------------

/// Seed and activate a minimal, command-only trait fixture at `id` under
/// `repo`, ready for `ctx traits run`. Mirrors
/// `flagged_and_configured_drives_preserve_identical_ledger_bytes`'s fixture
/// shape; local to this suite since no second suite needs a fixture builder.
fn seed_and_activate_fixture(repo: &std::path::Path, home: &std::path::Path, id: &str) {
    let fixture = repo
        .join(".ctx/traits")
        .join(id)
        .join("generated/index.toml");
    fs::create_dir_all(fixture.parent().unwrap()).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "ctx-proof-config"])
        .current_dir(repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "proof-config@example.invalid"])
        .current_dir(repo)
        .status()
        .unwrap();
    fs::write(
        &fixture,
        format!(
            r#"id = "{id}"
schema-version = "0.2"
version = "0.1.0"
name = "P428 fixture"
summary = "A provider-free pending ledger for build-cache proofs."

[procedure]
description = "Run one deterministic command."

[[slot]]
id = "notified"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Run command"
kind = "command"
cmd = "true"
output = ["slot:notified"]
"#
        ),
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits").join(id).join("trait.toml"),
        format!(
            "[package]\nid = \"{id}\"\nversion = \"0.1.0\"\nname = \"P428 fixture\"\nstatus = \"draft\"\n"
        ),
    )
    .unwrap();
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "initial"])
        .current_dir(repo)
        .status()
        .unwrap();

    assert_exit_code(
        &run_ctx(
            &[
                "traits",
                "review",
                "--file",
                fixture.to_str().unwrap(),
                "--approve",
            ],
            repo,
            home,
        ),
        0,
    );
    assert_exit_code(
        &run_ctx(
            &["traits", "activate", "--file", fixture.to_str().unwrap()],
            repo,
            home,
        ),
        0,
    );
}

#[test]
fn named_build_caches_export_concurrently_through_the_public_worktree_path() {
    let scratch = ScratchRoot::new("p428-worktree-env");
    let repo = scratch.home().join("repo");
    seed_and_activate_fixture(&repo, &scratch.home(), "p428-worktree-env");

    fs::write(
        repo.join(".ctx/config.toml"),
        "[worktree]\nsetup = [[\"/bin/sh\", \"-c\", \"printf '%s|%s' \\\"$CARGO_TARGET_DIR\\\" \\\"$PNPM_STORE\\\" > env-capture.txt\"]]\n\n\
[run.build-cache.cargo]\nenv = \"CARGO_TARGET_DIR\"\n\n\
[run.build-cache.pnpm]\nenv = \"PNPM_STORE\"\n",
    )
    .unwrap();

    let fixture = repo.join(".ctx/traits/p428-worktree-env/generated/index.toml");
    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            fixture.to_str().unwrap(),
            "--worktree=env-proof",
            "--no-drive",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);

    let captured = fs::read_to_string(repo.join(".ctx/worktrees/env-proof/env-capture.txt"))
        .expect("worktree setup command must have written env-capture.txt");
    let (cargo_dir, pnpm_dir) = captured
        .split_once('|')
        .unwrap_or_else(|| panic!("expected two '|'-separated values: {captured}"));
    assert!(!cargo_dir.is_empty(), "{captured}");
    assert!(!pnpm_dir.is_empty(), "{captured}");
    assert_ne!(
        cargo_dir, pnpm_dir,
        "two named build caches must resolve to distinct directories: {captured}"
    );
    assert!(cargo_dir.ends_with("/build/cargo"), "{captured}");
    assert!(pnpm_dir.ends_with("/build/pnpm"), "{captured}");
    assert!(
        cargo_dir.contains("/cache/") && pnpm_dir.contains("/cache/"),
        "both must resolve under the global per-repository cache root: {captured}"
    );
}

/// Extract the `path=...` value for `name`'s line from `ctx traits cache
/// prune --build[...]`'s plain-text output.
fn build_cache_report_path(stdout: &str, name: &str) -> String {
    stdout
        .lines()
        .find_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix(&format!("{name}: path="))?;
            rest.split(' ').next().map(str::to_string)
        })
        .unwrap_or_else(|| panic!("no {name:?} entry in prune output:\n{stdout}"))
}

#[test]
fn build_cache_prune_selects_one_or_all_declared_caches_and_never_touches_undeclared() {
    let scratch = ScratchRoot::new("p428-prune");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    fs::write(
        repo.join(".ctx/config.toml"),
        "[run.build-cache.cargo]\nenv = \"CARGO_TARGET_DIR\"\n\n[run.build-cache.pnpm]\nenv = \"PNPM_STORE\"\n",
    )
    .unwrap();

    // Learn the two declared caches' resolved paths without mutating
    // anything, then seed both plus an undeclared sibling directly under
    // their shared `build/` parent.
    let (dry_stdout, _) = utf8(&run_ctx(
        &["traits", "cache", "prune", "--build=cargo", "--dry-run"],
        &repo,
        &scratch.home(),
    ));
    let cargo_path = build_cache_report_path(&dry_stdout, "cargo");
    let build_root = std::path::Path::new(&cargo_path).parent().unwrap();
    let pnpm_path = build_root.join("pnpm");
    let undeclared_path = build_root.join("other-undeclared");
    for dir in [
        std::path::Path::new(&cargo_path),
        pnpm_path.as_path(),
        undeclared_path.as_path(),
    ] {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("marker.txt"), b"x").unwrap();
    }

    // Dry-run is non-mutating.
    assert_exit_code(
        &run_ctx(
            &["traits", "cache", "prune", "--build=cargo", "--dry-run"],
            &repo,
            &scratch.home(),
        ),
        0,
    );
    assert!(
        std::path::Path::new(&cargo_path).is_dir(),
        "dry-run must not remove anything"
    );

    // Named prune removes only the selected cache.
    assert_exit_code(
        &run_ctx(
            &["traits", "cache", "prune", "--build=cargo"],
            &repo,
            &scratch.home(),
        ),
        0,
    );
    assert!(
        !std::path::Path::new(&cargo_path).exists(),
        "named prune must remove cargo"
    );
    assert!(
        pnpm_path.exists(),
        "named prune must not touch other declared caches"
    );
    assert!(
        undeclared_path.exists(),
        "named prune must not touch an undeclared sibling"
    );

    // Bare `--build` prunes every declared cache, never the undeclared sibling.
    assert_exit_code(
        &run_ctx(
            &["traits", "cache", "prune", "--build"],
            &repo,
            &scratch.home(),
        ),
        0,
    );
    assert!(
        !pnpm_path.exists(),
        "bare --build must remove every declared cache"
    );
    assert!(
        undeclared_path.exists(),
        "bare --build must never scan or delete an undeclared directory"
    );

    // Ordinary metadata-only prune output is unaffected.
    let (plain_stdout, _) = utf8(&run_ctx(
        &["traits", "cache", "prune"],
        &repo,
        &scratch.home(),
    ));
    assert!(
        plain_stdout.contains("prune-mode: metadata-only"),
        "{plain_stdout}"
    );
}

#[test]
fn build_target_prune_reclaims_the_undeclared_legacy_cache_and_keeps_neighbors() {
    let scratch = ScratchRoot::new("p512-build-target-prune");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);

    let (dry_run, _) = utf8(&run_ctx(
        &["traits", "cache", "prune", "--build-target", "--dry-run"],
        &repo,
        &scratch.home(),
    ));
    let build_target = build_cache_report_path(&dry_run, "build-target");
    let cache_root = std::path::Path::new(&build_target).parent().unwrap();
    let neighbor = cache_root.join("neighbor");
    fs::create_dir_all(&build_target).unwrap();
    fs::write(std::path::Path::new(&build_target).join("marker"), "x").unwrap();
    fs::create_dir_all(&neighbor).unwrap();
    fs::write(neighbor.join("marker"), "x").unwrap();

    let (dry_run, _) = utf8(&run_ctx(
        &["traits", "cache", "prune", "--build-target", "--dry-run"],
        &repo,
        &scratch.home(),
    ));
    assert!(dry_run.contains(&build_target), "{dry_run}");
    assert!(dry_run.contains("bytes=1"), "{dry_run}");
    assert!(std::path::Path::new(&build_target).is_dir());

    assert_exit_code(
        &run_ctx(
            &["traits", "cache", "prune", "--build-target"],
            &repo,
            &scratch.home(),
        ),
        0,
    );
    assert!(!std::path::Path::new(&build_target).exists());
    assert!(
        neighbor.exists(),
        "legacy prune must retain neighboring paths"
    );
}

#[test]
fn linked_worktree_build_cache_export_doctor_and_prune_use_the_main_checkout_key() {
    let scratch = ScratchRoot::new("p512-linked-build-cache");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".ctx/config.toml"),
        "[run.build-cache.cargo]\nenv = \"CARGO_TARGET_DIR\"\n",
    )
    .unwrap();
    fs::write(repo.join("seed"), "seed").unwrap();
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["commit", "-qm", "seed"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );

    let linked = scratch.home().join("linked");
    assert!(
        Command::new("git")
            .args(["worktree", "add", "-qb", "linked", linked.to_str().unwrap()])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );

    let (main_dry_run, _) = utf8(&run_ctx(
        &["traits", "cache", "prune", "--build=cargo", "--dry-run"],
        &repo,
        &scratch.home(),
    ));
    let main_path = build_cache_report_path(&main_dry_run, "cargo");
    let build_root = std::path::Path::new(&main_path).parent().unwrap();
    let undeclared = build_root.join("undeclared");
    fs::create_dir_all(&main_path).unwrap();
    fs::write(std::path::Path::new(&main_path).join("marker"), "x").unwrap();
    fs::create_dir_all(&undeclared).unwrap();
    fs::write(undeclared.join("marker"), "x").unwrap();

    let (linked_dry_run, _) = utf8(&run_ctx(
        &["traits", "cache", "prune", "--build=cargo", "--dry-run"],
        &linked,
        &scratch.home(),
    ));
    assert_eq!(
        build_cache_report_path(&linked_dry_run, "cargo"),
        main_path,
        "linked-worktree prune must name the main checkout cache"
    );
    let (doctor, _) = utf8(&run_ctx(
        &["traits", "doctor", "--config"],
        &linked,
        &scratch.home(),
    ));
    assert!(doctor.contains(&main_path), "{doctor}");

    assert_exit_code(
        &run_ctx(
            &["traits", "cache", "prune", "--build=cargo"],
            &linked,
            &scratch.home(),
        ),
        0,
    );
    assert!(!std::path::Path::new(&main_path).exists());
    assert!(undeclared.exists(), "prune must retain undeclared siblings");
}

#[test]
fn linked_worktree_cache_export_uses_main_key_without_rebasing_relative_overlays() {
    let scratch = ScratchRoot::new("p512-linked-cache-env");
    let repo = scratch.home().join("repo");
    seed_and_activate_fixture(&repo, &scratch.home(), "p512-linked-cache-env");
    fs::write(
        repo.join(".ctx/config.toml"),
        "[worktree]\nsetup = [[\"/bin/sh\", \"-c\", \"printf '%s|%s' \\\"$CARGO_TARGET_DIR\\\" \\\"$RELATIVE_OVERLAY\\\" > env-capture.txt\"]]\n\n[worktree.env]\nRELATIVE_OVERLAY = \"./linked-local\"\n\n[run.build-cache.cargo]\nenv = \"CARGO_TARGET_DIR\"\n",
    )
    .unwrap();
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["commit", "-qm", "seed"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );

    let linked = scratch.home().join("linked");
    assert!(
        Command::new("git")
            .args(["worktree", "add", "-qb", "linked", linked.to_str().unwrap()])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    let (main_dry_run, _) = utf8(&run_ctx(
        &["traits", "cache", "prune", "--build=cargo", "--dry-run"],
        &repo,
        &scratch.home(),
    ));
    let main_cache = build_cache_report_path(&main_dry_run, "cargo");

    let fixture = linked.join(".ctx/traits/p512-linked-cache-env/generated/index.toml");
    assert_exit_code(
        &run_ctx(
            &[
                "traits",
                "run",
                "--file",
                fixture.to_str().unwrap(),
                "--worktree=env-proof",
                "--no-drive",
            ],
            &linked,
            &scratch.home(),
        ),
        0,
    );
    let captured = fs::read_to_string(linked.join(".ctx/worktrees/env-proof/env-capture.txt"))
        .expect("linked worktree setup must capture effective environment");
    let (cache, relative) = captured.split_once('|').expect("two captured values");
    assert_eq!(cache, main_cache);
    let linked_real = fs::canonicalize(&linked).unwrap();
    assert_eq!(
        relative,
        linked_real.join("./linked-local").to_str().unwrap(),
        "ordinary relative overlays must retain the invocation checkout root"
    );
}

#[test]
fn build_cache_prune_authority_is_rooted_at_selected_repo_not_undeclared_names() {
    let scratch = ScratchRoot::new("p428-prune-authority");
    let repo_a = scratch.home().join("repo-a");
    let repo_b = scratch.home().join("repo-b");
    for repo in [&repo_a, &repo_b] {
        fs::create_dir_all(repo.join(".ctx")).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .status()
            .unwrap();
    }
    fs::write(
        repo_a.join(".ctx/config.toml"),
        "[run.build-cache.repo-a-only]\nenv = \"REPO_A_ONLY\"\n",
    )
    .unwrap();
    fs::write(
        repo_b.join(".ctx/config.toml"),
        "[run.build-cache.repo-b-only]\nenv = \"REPO_B_ONLY\"\n",
    )
    .unwrap();

    // From inside repo A, `--repo-root repo-b --build` must resolve
    // declarations from repo B (the explicitly selected repository), not
    // from repo A (the invocation CWD).
    let (dry_stdout, _) = utf8(&run_ctx(
        &[
            "traits",
            "cache",
            "prune",
            "--repo-root",
            repo_b.to_str().unwrap(),
            "--build",
            "--dry-run",
        ],
        &repo_a,
        &scratch.home(),
    ));
    assert!(
        dry_stdout.contains("repo-b-only"),
        "prune --repo-root <repo-b> --build must report repo B's declared cache:\n{dry_stdout}"
    );
    assert!(
        !dry_stdout.contains("repo-a-only"),
        "prune --repo-root <repo-b> --build must not report repo A's declared cache:\n{dry_stdout}"
    );

    // An explicit but undeclared name must fail before touching disk, and
    // must never delete an existing undeclared sibling directory.
    let repo_b_cache_path = build_cache_report_path(&dry_stdout, "repo-b-only");
    let build_root = std::path::Path::new(&repo_b_cache_path).parent().unwrap();
    let undeclared_path = build_root.join("not-declared-anywhere");
    fs::create_dir_all(&undeclared_path).unwrap();
    fs::write(undeclared_path.join("marker.txt"), b"x").unwrap();

    let output = run_ctx(
        &[
            "traits",
            "cache",
            "prune",
            "--repo-root",
            repo_b.to_str().unwrap(),
            "--build=not-declared-anywhere",
        ],
        &repo_a,
        &scratch.home(),
    );
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(stderr.contains("not-declared-anywhere"), "{stderr}");
    assert!(
        undeclared_path.exists(),
        "rejecting an undeclared name must never delete it"
    );
}

#[test]
fn build_cache_prune_rejects_cache_root_override() {
    let scratch = ScratchRoot::new("p428-prune-cache-root-conflict");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    let output = run_ctx(
        &[
            "traits",
            "cache",
            "prune",
            "--build",
            "--cache-root",
            "/tmp/wherever",
        ],
        &repo,
        &scratch.home(),
    );
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(stderr.contains("--cache-root"), "{stderr}");
}
