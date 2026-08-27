//! PTY coverage for alternate-screen run-view teardown paths.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

use support::{
    ScratchRoot, ctx_bin, git_init, raw_after_terminal_restore, require_success,
    run_pty_keys_after_markers, run_pty_signal_after_marker, run_pty_with_cursor_reply, spawn_ctx,
    text_after_terminal_restore,
};

const TRAIT_ID: &str = "demo";
const TRAIT_PATH: &str = ".ctx/traits/demo/generated/index.toml";
const STEP_ID: &str = "only-step";
const CLEAR_VIEWPORT: &str = "\x1b[H\x1b[2J";
const ENTER_ALT: &str = "\x1b[?1049h";
const LEAVE_ALT: &str = "\x1b[?1049l";
const RESCUE_ERROR_TEXT: &str = "interrupted (signal)";

struct ExitFixture {
    _scratch: ScratchRoot,
    repo: PathBuf,
    home: PathBuf,
}

fn command_trait_fixture(label: &str, command: &str) -> ExitFixture {
    let scratch = ScratchRoot::new("p252-run-view-exit");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init(&repo);
    fs::write(
        repo.join(".gitignore"),
        ".ctx/traits/worktrees/\n.ctx/runs/\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        format!(
            "id = \"demo\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"{label}\"\nsummary = \"Demo\"\n\n[procedure]\ndescription = \"Run command\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[[procedure.sequence]]\nid = \"{STEP_ID}\"\ntitle = \"{STEP_ID}\"\nkind = \"command\"\ncmd = \"{command}\"\noutput = [\"slot:notified\"]\n"
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
    require_success(
        "approve fixture",
        &["traits", "trust", "--approved", TRAIT_PATH],
        &repo,
        &home,
    );
    require_success(
        "activate fixture",
        &["traits", "state", "--active", "--file", TRAIT_PATH],
        &repo,
        &home,
    );
    ExitFixture {
        _scratch: scratch,
        repo,
        home,
    }
}

struct BackgroundRun(Child);

impl Drop for BackgroundRun {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_background_run(fixture: &ExitFixture) -> BackgroundRun {
    BackgroundRun(spawn_ctx(
        &[
            "traits",
            "run",
            "--progress",
            "none",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
        ],
        &fixture.repo,
        &fixture.home,
    ))
}

fn ledger_session_id(repo: &Path) -> String {
    for _ in 0..30 {
        let ledger = fs::read_dir(repo.parent().unwrap().join("ctx/traits/runs"))
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|entry| fs::read_dir(entry.path()).into_iter().flatten().flatten())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
                    && !path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().ends_with(".summary.json"))
            })
            .collect::<Vec<_>>();
        if ledger.len() == 1 {
            return serde_json::from_str::<serde_json::Value>(
                &fs::read_to_string(&ledger[0]).unwrap(),
            )
            .unwrap()["session-id"]
                .as_str()
                .expect("run ledger has a session-id")
                .to_string();
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("expected exactly one run ledger in {}", repo.display());
}

fn assert_only_interrupted_panel(raw: &str, trait_id: &str, session_id: &str) {
    let text = support::strip_escapes(raw_after_terminal_restore(raw));
    let lines = text
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty() && !line.contains("CHILD_EXIT__"))
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            format!("┌── {trait_id}"),
            format!("│   session: {session_id}"),
            format!("│   error:   {RESCUE_ERROR_TEXT}"),
            "└── Failure".to_string(),
        ],
        "unexpected surviving screen: {text:?}"
    );
    assert!(!raw_after_terminal_restore(raw).contains(CLEAR_VIEWPORT));
}

#[test]
fn sigterm_leaves_only_the_interrupted_failure_panel() {
    let fixture = command_trait_fixture("sigterm", "sleep 30");
    let (code, raw) = run_pty_signal_after_marker(
        &ctx_bin(),
        "traits --session .ctx/runs/signal.json run --progress tui --file .ctx/traits/demo/generated/index.toml",
        &fixture.repo,
        &fixture.home,
        STEP_ID,
        "TERM",
        1,
    );
    assert_eq!(code, 143);
    assert!(raw.contains(ENTER_ALT) && raw.contains(LEAVE_ALT) && raw.contains("\x1b[?25h"));
    assert_only_interrupted_panel(&raw, TRAIT_ID, &ledger_session_id(&fixture.repo));
}

#[test]
fn repeated_sigint_leaves_only_the_interrupted_failure_panel() {
    let fixture = command_trait_fixture("sigint", "sleep 30");
    let (code, raw) = run_pty_signal_after_marker(
        &ctx_bin(),
        "traits --session .ctx/runs/signal.json run --progress tui --file .ctx/traits/demo/generated/index.toml",
        &fixture.repo,
        &fixture.home,
        STEP_ID,
        "INT",
        3,
    );
    assert_eq!(code, 130);
    assert!(raw.contains(ENTER_ALT));
    assert_only_interrupted_panel(&raw, TRAIT_ID, &ledger_session_id(&fixture.repo));
}

#[test]
fn panic_mid_run_leaves_no_frame_rows() {
    let fixture = command_trait_fixture("panic", "sleep 30");
    let args = format!(
        "CTX_INTERNAL_TESTHOOK_PANIC_AFTER_RUN_VIEW_RENDER=1 {} traits --session .ctx/runs/panic.json run --progress tui --file .ctx/traits/demo/generated/index.toml",
        ctx_bin().display()
    );
    let (code, raw) = run_pty_with_cursor_reply(
        std::path::Path::new("/usr/bin/env"),
        &args,
        &fixture.repo,
        &fixture.home,
        "__PANIC_COMPLETE__",
        ".ctx/panic-termios",
    );
    assert_ne!(code, 0);
    assert!(raw.contains(ENTER_ALT) && raw.contains(LEAVE_ALT));
    assert!(raw.find(STEP_ID).unwrap() < raw.rfind(LEAVE_ALT).unwrap());
    let restored = text_after_terminal_restore(&raw);
    assert!(
        restored.contains("test hook: panic after run-view render"),
        "panic diagnostic was not printed after restore: {raw:?}"
    );
    assert!(!restored.contains(STEP_ID));
    assert!(!raw.contains(RESCUE_ERROR_TEXT));
    assert!(!raw_after_terminal_restore(&raw).contains(CLEAR_VIEWPORT));
}

#[test]
fn dashboard_attach_then_exit_leaves_no_run_frame() {
    let fixture = command_trait_fixture("dashboard", "sleep 30");
    let background = spawn_background_run(&fixture);
    thread::sleep(Duration::from_secs(1));
    let (code, raw) = run_pty_keys_after_markers(
        &ctx_bin(),
        "traits",
        &fixture.repo,
        &fixture.home,
        &[
            // Clamp selection to the top, then move to the first live session.
            ("session-", "kkkkkkkkj\r"),
            (r"(?s)only-step.*\[d\] dash", "q"),
            ("Quit live view?", "\r"),
            ("SESSIONS", "q"),
            (r"Quit.*ctx.*traits\?", "\r"),
        ],
    );
    drop(background);
    assert_eq!(code, 0, "dashboard output: {raw:?}");
    assert!(!text_after_terminal_restore(&raw).contains(STEP_ID));
}

#[test]
fn clean_run_teardown_discards_the_live_frame_and_prints_the_final_panel() {
    let fixture = command_trait_fixture("clean", "true");
    let (code, raw) = run_pty_with_cursor_reply(
        &ctx_bin(),
        "traits --session .ctx/runs/clean.json run --progress tui --file .ctx/traits/demo/generated/index.toml",
        &fixture.repo,
        &fixture.home,
        "__RUN_COMPLETE__",
        ".ctx/termios",
    );
    assert_eq!(code, 0);
    assert!(raw.contains(ENTER_ALT) && raw.contains(LEAVE_ALT));
    assert!(raw.find(STEP_ID).unwrap() < raw.rfind(LEAVE_ALT).unwrap());
    let final_text = text_after_terminal_restore(&raw);
    assert!(final_text.contains("┌── "));
    assert!(final_text.contains(&format!("session: {}", ledger_session_id(&fixture.repo))));
    assert!(final_text.contains("└── Success"));
    assert!(!final_text.contains(STEP_ID));
    assert!(!final_text.contains(RESCUE_ERROR_TEXT));
    assert!(!raw_after_terminal_restore(&raw).contains(CLEAR_VIEWPORT));
}

#[test]
fn ctrl_c_during_a_live_run_restores_the_screen_before_the_kill_note() {
    let fixture = command_trait_fixture("ctrl-c", "sleep 30");
    let (code, raw) = run_pty_keys_after_markers(
        &ctx_bin(),
        "traits --session .ctx/runs/ctrl-c.json run --progress tui --file .ctx/traits/demo/generated/index.toml",
        &fixture.repo,
        &fixture.home,
        &[(STEP_ID, "\u{3}")],
    );
    assert_ne!(code, 0);
    assert!(raw.contains(ENTER_ALT) && raw.contains(LEAVE_ALT));
    let restored = text_after_terminal_restore(&raw);
    assert!(
        restored
            .lines()
            .any(|line| line.trim() == "run killed; terminal restored")
    );
    assert!(!restored.contains(STEP_ID));
    assert!(
        restored
            .lines()
            .filter(|line| line.contains('│'))
            .all(|line| line.starts_with("│   "))
    );
    assert!(!raw_after_terminal_restore(&raw).contains(CLEAR_VIEWPORT));
}
