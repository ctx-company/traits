//! P453/P461: the release CLI surface — `ctx traits help --json` and
//! `ctx traits -h` enumerate the mandated visible command set (in order,
//! with the correct hidden compatibility aliases), every visible command
//! reports its own `--json` flag, hidden commands still run under
//! `--help`, and the `export`/`render` and `vendor`/`sync` alias pairs (plus
//! namespace-level `--json` on `cache`/`trust`) are behaviorally equivalent.
//! Migrated from `scripts/byte_compare.rs`'s `--cli-surface-proof`.

use std::fs;

use support::{ScratchRoot, git_init, require_success};

/// Write `contents` to `path`, creating parent directories as needed. Local
/// to this suite: promote to `support` if a second suite needs it.
fn write_fixture_file(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    std::fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

const VISIBLE_COMMANDS: &[&str] = &[
    "init",
    "new",
    "list",
    "build",
    "check",
    "diff",
    "explain",
    "generate",
    "refine",
    "critique",
    "migrate",
    "export",
    "host",
    "run",
    "merge",
    "activate",
    "trust",
    "dependency",
    "import",
    "doctor",
    "cache",
    "config",
];

const HIDDEN_NAMES_NEVER_VISIBLE: &[&str] = &[
    "render",
    "sync",
    "eval",
    "generate-evals",
    "prompt",
    "session",
];

/// Pinned literally (not read from the product's own tagline constant) so
/// this test fails if the release tagline drifts from the required wording
/// rather than only checking `-h` and `help --json` agree with each other.
const TAGLINE: &str = "typed, digest-locked agent procedures you can prove, reproduce, and gate";

const DEMO_TRAIT_MANIFEST: &str = "id = \"cli-surface-demo\"\n\
schema-version = \"0.2\"\n\
version = \"0.1.0\"\n\
name = \"CLI Surface Demo\"\n\
summary = \"P453 CLI-surface structural/equivalence proof fixture.\"\n";

/// Extract every top-level command row's `name` and `hidden` field from a
/// `help --json` document's `commands` array as `(name, hidden)` pairs,
/// without a JSON dependency: walk brace depth and only record fields seen
/// at `target_depth` (2 for the top-level command rows, one level deeper
/// than the array itself), pairing each `name` with the `hidden` value that
/// follows it at the same depth before that command's own nested
/// `flags`/`subcommands` arrays push depth past `target_depth` again.
fn json_top_level_commands(json: &str, target_depth: i32) -> Vec<(String, bool)> {
    let mut rows = Vec::new();
    let mut depth = 0_i32;
    let mut pending_name: Option<String> = None;
    let bytes = json.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b'"' if depth == target_depth && json[idx..].starts_with("\"name\"") => {
                if let Some(value) = json_string_field_value(&json[idx..]) {
                    pending_name = Some(value);
                }
            }
            b'"' if depth == target_depth && json[idx..].starts_with("\"hidden\"") => {
                if let Some(name) = pending_name.take() {
                    let after = &json[idx + "\"hidden\"".len()..];
                    let is_hidden = after
                        .split(':')
                        .nth(1)
                        .is_some_and(|value| value.trim_start().starts_with("true"));
                    rows.push((name, is_hidden));
                }
            }
            _ => {}
        }
        idx += 1;
    }
    rows
}

fn json_string_field_value(text: &str) -> Option<String> {
    let colon = text.find(':')?;
    let after_colon = text[colon + 1..].trim_start();
    let rest = after_colon.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Whether the top-level `help --json` command row named `name` reports its
/// own `--json` flag, scoped to that command's own `"flags"` array (the
/// first one found after its `"name"` marker — by `SurfaceCommand`'s field
/// order `name, group, hidden, about, aliases, flags, subcommands`, always
/// this command's own, never a nested subcommand's).
fn top_level_command_has_json_flag(json: &str, name: &str) -> bool {
    let marker = format!("\"name\": \"{name}\"");
    let Some(start) = json.find(&marker) else {
        return false;
    };
    let Some(flags_rel) = json[start..].find("\"flags\": [") else {
        return false;
    };
    let flags_start = start + flags_rel + "\"flags\": [".len();
    let bytes = json.as_bytes();
    let mut depth = 1_i32;
    let mut idx = flags_start;
    while idx < bytes.len() && depth > 0 {
        match bytes[idx] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            _ => {}
        }
        idx += 1;
    }
    json[flags_start..idx].contains("\"long\": \"json\"")
}

#[test]
fn help_json_enumerates_required_visible_surface_with_aliases() {
    let scratch = ScratchRoot::new("cli-surface-help-json");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let help_json = require_success(
        "`ctx traits help --json`",
        &["traits", "help", "--json"],
        &repo,
        &scratch.home(),
    );
    assert!(
        help_json.trim_start().starts_with('{'),
        "`help --json` did not emit a JSON object:\n{help_json}"
    );
    let tagline_marker = format!("\"tagline\": \"{TAGLINE}\"");
    assert!(
        help_json.contains(&tagline_marker),
        "`help --json` does not report the mandated tagline {TAGLINE:?}:\n{help_json}"
    );

    let top_level_commands = json_top_level_commands(&help_json, 2);
    let top_level_names: Vec<String> = top_level_commands
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    let mut sorted_expected = VISIBLE_COMMANDS.to_vec();
    sorted_expected.sort_unstable();
    let mut sorted_actual: Vec<&str> = top_level_commands
        .iter()
        .filter(|(_, hidden)| !hidden)
        .map(|(name, _)| name.as_str())
        .collect();
    sorted_actual.sort_unstable();
    sorted_actual.dedup();
    assert_eq!(
        sorted_actual, sorted_expected,
        "visible command set mismatch"
    );

    // The one surviving visible-command alias pair. P567 retired the other
    // (`vendor`/`sync`): `vendor` is itself hidden now, so the pair no longer
    // describes two spellings of a VISIBLE command — `dependency install` is
    // the visible spelling, and `vendor`'s own alias is covered by the
    // hidden-command checks instead.
    let (canonical, alias) = ("export", "render");
    let canonical_marker = format!("\"name\": \"{canonical}\"");
    let alias_marker = format!("\"{alias}\"");
    let canonical_start = help_json
        .find(&canonical_marker)
        .unwrap_or_else(|| panic!("`help --json` is missing canonical command {canonical:?}"));
    let window_end = (canonical_start + 2000).min(help_json.len());
    assert!(
        help_json[canonical_start..window_end].contains(&alias_marker),
        "{canonical:?} is missing the hidden compatibility alias {alias:?} in `help --json`"
    );
    assert!(
        !top_level_names.iter().any(|name| name == alias),
        "{alias:?} must not appear as its own top-level command distinct from {canonical:?}"
    );

    let missing_json: Vec<&str> = VISIBLE_COMMANDS
        .iter()
        .copied()
        .filter(|name| !top_level_command_has_json_flag(&help_json, name))
        .collect();
    assert!(
        missing_json.is_empty(),
        "visible commands missing their own `--json` flag: {missing_json:?}"
    );
}

#[test]
fn dash_h_is_one_screen_and_matches_bare_help() {
    let scratch = ScratchRoot::new("cli-surface-dash-h");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let help_text = require_success("`ctx traits -h`", &["traits", "-h"], &repo, &scratch.home());
    assert!(
        help_text.contains(TAGLINE),
        "`ctx traits -h` does not contain the mandated tagline {TAGLINE:?}:\n{help_text}"
    );

    // Bare `ctx traits help` (the hidden explicit `Help` variant replacing
    // clap's generated `help` subcommand) renders byte-identical release
    // help to `ctx traits -h`, not top-level `ctx` help.
    let bare_help_text = require_success(
        "`ctx traits help`",
        &["traits", "help"],
        &repo,
        &scratch.home(),
    );
    assert_eq!(
        bare_help_text, help_text,
        "`ctx traits help` diverged from `ctx traits -h`"
    );

    let row_count = help_text.lines().count();
    assert!(
        row_count <= 40,
        "`ctx traits -h` is {row_count} rows, exceeding the required 40-row one-screen limit"
    );

    fn first_word(line: &str) -> Option<&str> {
        line.split_whitespace().next()
    }
    for name in VISIBLE_COMMANDS {
        assert!(
            help_text.lines().any(|line| first_word(line) == Some(name)),
            "`ctx traits -h` is missing required visible command {name:?}"
        );
    }
    for hidden_name in HIDDEN_NAMES_NEVER_VISIBLE {
        assert!(
            !help_text
                .lines()
                .any(|line| first_word(line) == Some(hidden_name)),
            "`ctx traits -h` must not list hidden command/alias {hidden_name:?}"
        );
    }
}

#[test]
fn hidden_commands_still_run_under_help() {
    let scratch = ScratchRoot::new("cli-surface-hidden-help");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let help_json = require_success(
        "`ctx traits help --json`",
        &["traits", "help", "--json"],
        &repo,
        &scratch.home(),
    );
    // Discovered from `help --json` (not a hand-picked subset) so this test
    // cannot silently omit a newly hidden command.
    for (name, _) in json_top_level_commands(&help_json, 2) {
        if VISIBLE_COMMANDS.contains(&name.as_str()) {
            continue;
        }
        require_success(
            &format!("`ctx traits {name} --help`"),
            &["traits", &name, "--help"],
            &repo,
            &scratch.home(),
        );
    }
}

#[test]
fn build_and_refine_help_advertise_trait_names_with_path_escape_hatches() {
    let scratch = ScratchRoot::new("cli-surface-build-refine-operands");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();

    for command in ["build", "refine"] {
        let help = require_success(
            &format!("`ctx traits {command} --help`"),
            &["traits", command, "--help"],
            &repo,
            &scratch.home(),
        );
        assert!(
            help.contains("<TRAIT>"),
            "{command} help must advertise a trait operand:\n{help}"
        );
        assert!(
            help.contains("explicit") && help.contains("escape hatch"),
            "{command} help must document explicit paths as an escape hatch:\n{help}"
        );
    }
}

fn seed_demo_trait(repo: &std::path::Path) {
    git_init(repo);
    write_fixture_file(
        &repo.join(".ctx/traits/cli-surface-demo/trait.toml"),
        "[package]\nid = \"cli-surface-demo\"\nversion = \"0.1.0\"\nname = \"CLI Surface Demo\"\nstatus = \"draft\"\n",
    );
    write_fixture_file(
        &repo.join(".ctx/traits/cli-surface-demo/generated/index.toml"),
        DEMO_TRAIT_MANIFEST,
    );
}

#[test]
fn export_and_render_alias_write_identical_artifacts() {
    let scratch = ScratchRoot::new("cli-surface-export-render");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    seed_demo_trait(&repo);

    let canonical_out = repo.join("export-canonical");
    let alias_out = repo.join("export-alias");
    let canonical_stdout = require_success(
        "`ctx traits export --json`",
        &[
            "traits",
            "export",
            "cli-surface-demo",
            "--out",
            canonical_out.to_str().unwrap(),
            "--allow-unreviewed",
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    let alias_stdout = require_success(
        "`ctx traits render --json` (hidden alias of export)",
        &[
            "traits",
            "render",
            "cli-surface-demo",
            "--out",
            alias_out.to_str().unwrap(),
            "--allow-unreviewed",
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    let normalized_alias_stdout =
        alias_stdout.replace(alias_out.to_str().unwrap(), canonical_out.to_str().unwrap());
    assert_eq!(
        normalized_alias_stdout, canonical_stdout,
        "`render` and `export` diverged on identical input"
    );

    let canonical_skill = fs::read(canonical_out.join("cli-surface-demo/SKILL.md")).unwrap();
    let alias_skill = fs::read(alias_out.join("cli-surface-demo/SKILL.md")).unwrap();
    assert_eq!(
        canonical_skill, alias_skill,
        "`render` and `export` wrote different SKILL.md bytes"
    );
}

#[test]
fn vendor_and_sync_alias_write_identical_lock_bytes() {
    let scratch = ScratchRoot::new("cli-surface-vendor-sync");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    seed_demo_trait(&repo);

    let vendor_stdout = require_success(
        "`ctx traits vendor --json`",
        &["traits", "vendor", "cli-surface-demo", "--json"],
        &repo,
        &scratch.home(),
    );
    let lock_path = repo.join(".ctx/traits/cli-surface-demo/trait.lock");
    let vendor_lock = fs::read(&lock_path).unwrap();
    fs::remove_file(&lock_path).unwrap();

    let sync_stdout = require_success(
        "`ctx traits sync --json` (hidden alias of vendor)",
        &["traits", "sync", "cli-surface-demo", "--json"],
        &repo,
        &scratch.home(),
    );
    assert_eq!(
        sync_stdout, vendor_stdout,
        "`sync` and `vendor` diverged on identical input"
    );
    let sync_lock = fs::read(&lock_path).unwrap();
    assert_eq!(
        vendor_lock, sync_lock,
        "`sync` and `vendor` wrote different trait.lock bytes"
    );
}

/// P490 Guard 3: a package-local `trait.lock`'s `[[dependency]]` entry must
/// record a repo-relative `vendored-path` (matching the project-level
/// `.ctx/traits.lock` doctrine `scripts/byte_compare.rs` already asserts for
/// npm-installed deps), never the authoring machine's absolute path.
#[test]
fn vendor_records_repo_relative_dependency_vendored_path() {
    let scratch = ScratchRoot::new("cli-surface-dependency-path");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    write_fixture_file(
        &repo.join(".ctx/traits/dep-pkg/trait.toml"),
        "[package]\nid = \"dep-pkg\"\nversion = \"0.1.0\"\nname = \"Dep Pkg\"\nstatus = \"draft\"\n",
    );
    write_fixture_file(
        &repo.join(".ctx/traits/dep-pkg/generated/index.toml"),
        "id = \"dep-pkg\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"Dep Pkg\"\nsummary = \"P490 dependency-path fixture.\"\n",
    );
    write_fixture_file(
        &repo.join(".ctx/traits/main-pkg/trait.toml"),
        "[package]\nid = \"main-pkg\"\nversion = \"0.1.0\"\nname = \"Main Pkg\"\nstatus = \"draft\"\n\n\
         [dependencies]\ndep-pkg = { version = \"0.1.0\", path = \"../dep-pkg\" }\n",
    );
    write_fixture_file(
        &repo.join(".ctx/traits/main-pkg/generated/index.toml"),
        "id = \"main-pkg\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"Main Pkg\"\nsummary = \"P490 dependency-path fixture.\"\n",
    );

    require_success(
        "`ctx traits vendor --json` for a package with a path `[dependencies]` entry",
        &["traits", "vendor", "main-pkg", "--json"],
        &repo,
        &scratch.home(),
    );

    let lock_text = fs::read_to_string(repo.join(".ctx/traits/main-pkg/trait.lock")).unwrap();
    assert!(
        lock_text.contains("vendored-path = \".ctx/traits/vendor/dep-pkg\""),
        "package-local trait.lock does not record a repository-relative vendored-path: {lock_text}"
    );
    assert!(
        !lock_text.contains(repo.to_str().unwrap()),
        "package-local trait.lock leaks an absolute repo path into vendored-path: {lock_text}"
    );
}

#[test]
fn namespace_level_json_flag_matches_subcommand_level_flag() {
    let scratch = ScratchRoot::new("cli-surface-namespace-json");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    seed_demo_trait(&repo);
    let repo_str = repo.to_str().unwrap();

    let cache_json_after = require_success(
        "`ctx traits cache status --json`",
        &[
            "traits",
            "cache",
            "status",
            "--repo-root",
            repo_str,
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    let cache_json_before = require_success(
        "`ctx traits cache --json status` (namespace-level flag)",
        &[
            "traits",
            "cache",
            "--json",
            "status",
            "--repo-root",
            repo_str,
        ],
        &repo,
        &scratch.home(),
    );
    assert_eq!(
        cache_json_before, cache_json_after,
        "`ctx traits cache --json status` diverged from `ctx traits cache status --json`"
    );

    let trust_after_home = ScratchRoot::new("cli-surface-trust-after");
    let trust_json_after = require_success(
        "`ctx traits trust approve --json`",
        &["traits", "trust", "approve", "cli-surface-demo", "--json"],
        &repo,
        &trust_after_home.home(),
    );
    let trust_before_home = ScratchRoot::new("cli-surface-trust-before");
    let trust_json_before = require_success(
        "`ctx traits trust --json approve` (namespace-level flag)",
        &["traits", "trust", "--json", "approve", "cli-surface-demo"],
        &repo,
        &trust_before_home.home(),
    );
    let mut trust_before: serde_json::Value = serde_json::from_str(&trust_json_before).unwrap();
    let mut trust_after: serde_json::Value = serde_json::from_str(&trust_json_after).unwrap();
    for value in [&mut trust_before, &mut trust_after] {
        value.as_object_mut().unwrap().remove("path");
        value.as_object_mut().unwrap().remove("seq");
    }
    assert_eq!(
        trust_before, trust_after,
        "trust JSON semantics diverged by flag placement"
    );
}
