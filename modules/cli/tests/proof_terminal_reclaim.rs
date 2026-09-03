//! Public-path proof for terminal reclaim when `drive()` records a killed run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use support::{ScratchRoot, git_init_on_branch, require_success, run_ctx_with_env, utf8};

fn init_fixture(repo: &Path, home: &Path) {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init_on_branch(repo, "main");
    fs::write(
        repo.join(".gitignore"),
        ".ctx/traits/worktrees/\ncache-path\ncheap-cache\nexpensive-cache\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        "[worktree]\nsetup = [[\"sh\", \"-c\", \"mkdir -p \\\"$SLOT_DIR\\\" \\\"$BUILD_CACHE\\\" cheap-cache expensive-cache && touch \\\"$SLOT_DIR/build-output\\\" \\\"$BUILD_CACHE/named-output\\\" cheap-cache/blob expensive-cache/blob && printf '%s' \\\"$BUILD_CACHE\\\" > cache-path\"]]\n\n[worktree.env]\nSLOT_DIR = \"{cache-slot}\"\n\n[worktree.build-cache.terminal-proof]\nenv = \"BUILD_CACHE\"\n\n[worktree.retention]\ncheap = [\"cheap-cache\"]\nexpensive = [\"expensive-cache\"]\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        "id = \"demo\"\nschema-version = \"0.4\"\nversion = \"0.1.0\"\nname = \"Demo\"\ndescription = \"A provider-free command-only trait.\"\n\n[procedure]\ndescription = \"Run one deterministic command.\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"true\"\noutput = [\"slot:notified\"]\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(repo)
        .status()
        .unwrap();
    let fixture = ".ctx/traits/demo/generated/index.toml";
    require_success(
        "review",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        repo,
        home,
    );
    require_success(
        "activate",
        &["traits", "state", "--active", "--file", fixture],
        repo,
        home,
    );
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "activate"])
        .current_dir(repo)
        .status()
        .unwrap();
}

fn value_json(output: &std::process::Output) -> serde_json::Value {
    let (stdout, _) = utf8(output);
    let start = stdout
        .lines()
        .position(|line| line.trim_start().starts_with('{'))
        .unwrap();
    serde_json::from_str(&stdout.lines().skip(start).collect::<Vec<_>>().join("\n")).unwrap()
}

fn slots_registry(home: &Path) -> PathBuf {
    let mut directories = vec![home.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == "slots.json") {
                return path;
            }
            if entry.file_type().unwrap().is_dir() {
                directories.push(path);
            }
        }
    }
    panic!("fixture did not create a slot registry");
}

#[test]
fn killed_run_reclaims_terminal_artifacts_and_records_evidence() {
    let scratch = ScratchRoot::new("terminal-reclaim-killed");
    let home = scratch.home();
    let repo = home.join("repo");
    init_fixture(&repo, &home);

    // This debug-only hook changes the completed loop result before the shared
    // post-loop terminal sequence, so reclaim and outcome persistence run normally.
    let output = run_ctx_with_env(
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
        &repo,
        &home,
        &[("CTX_INTERNAL_TESTHOOK_FORCE_KILLED_OUTCOME", "1")],
    );
    assert!(
        output.status.success(),
        "the run envelope remains successful while its recorded drive is killed: {}",
        utf8(&output).1
    );
    let envelope = value_json(&output);
    let session_path = PathBuf::from(envelope["value"]["session-path"].as_str().unwrap());
    let worktree = fs::read_dir(repo.join(".ctx/traits/worktrees"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let cache = PathBuf::from(fs::read_to_string(worktree.join("cache-path")).unwrap());
    let registry = slots_registry(&home);

    assert!(!worktree.join("cheap-cache").exists());
    assert!(!worktree.join("expensive-cache").exists());
    assert!(!cache.exists());
    let registry_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry).unwrap()).unwrap();
    assert!(
        registry_after["assignments"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(worktree.exists(), "a killed run retains its worktree");

    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(session_path).unwrap()).unwrap();
    let branch = ledger["provenance"]["worktree"]["branch"].as_str().unwrap();
    assert!(
        Command::new("git")
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}")
            ])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success(),
        "a killed run retains its branch"
    );
    let reclaim = &ledger["last-drive-outcome"]["reclaim"];
    assert_eq!(ledger["last-drive-outcome"]["outcome"], "killed");
    assert_eq!(reclaim["worktree-paths"].as_array().unwrap().len(), 2);
    assert!(
        reclaim["worktree-paths"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["removed"] == true)
    );
    assert!(reclaim["slot"].get("released").is_some());
    assert!(
        registry
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("slot-"))
            .all(|entry| !entry.path().join("build-output").exists()),
        "killed reclaim must remove all owned slot bytes"
    );
    assert!(reclaim["named-caches"][0].get("deleted").is_some());
}
