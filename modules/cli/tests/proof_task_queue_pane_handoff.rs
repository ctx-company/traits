//! PTY coverage for 0199: a `--task` queue run must bring up the inline
//! run pane for EVERY member, never falling back to status progress because
//! a fresh cursor query raced the previous member's (or the startup pane's)
//! still-draining input pump. Reuses the expect/PTY recipe and fixture
//! shape `proof_task_queue_refusal_teardown.rs` already established for the
//! `--task` queue path (0198), rather than reimplementing it.

use std::fs;
use std::process::Command;

use support::{
    ScratchRoot, ctx_bin, git_init, require_success, run_pty_with_cursor_reply,
    text_after_terminal_restore,
};

struct Fixture {
    _scratch: ScratchRoot,
    repo: std::path::PathBuf,
    home: std::path::PathBuf,
}

/// A `demo` trait that declares a `task-board` resource and a single
/// `cmd = "false"` step, dispatched to by `runtime.toml`, plus two
/// independent `ready` tasks. Each queue member's command step is rejected
/// fast — non-interactive, no stdin to approve it — so no worker harness
/// and no merge machinery is ever exercised, but `create_run_panel`
/// (`drive.rs`) still builds a fresh inline pane per member first, which is
/// exactly the handoff window 0199 closes.
fn failing_two_member_queue_fixture() -> Fixture {
    let scratch = ScratchRoot::new("p0199-queue-pane-handoff");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/authored/demo/generated")).unwrap();
    fs::create_dir_all(repo.join(".internal/tasks")).unwrap();
    git_init(&repo);
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/demo/generated/index.toml"),
        "id = \"demo\"\nschema-version = \"0.4\"\nversion = \"0.1.0\"\nname = \"Demo\"\ndescription = \"Demo trait with a task-board resource and a step that always fails fast.\"\n\n[[resource]]\nid = \"task-board\"\npath = \".internal/tasks\"\nroot = \"repo\"\ntrigger = \"on-demand\"\n\n[[port]]\nid = \"task\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Task to implement.\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[procedure]\ndescription = \"Run command\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"false\"\noutput = [\"slot:notified\"]\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    for key in ["0002", "0003"] {
        fs::write(
            repo.join(format!(".internal/tasks/{key}-demo.toml")),
            format!(
                "schema-version = \"0.2\"\nkey = \"{key}\"\ntitle = \"Scratch pane-handoff fixture\"\nstatus = \"ready\"\nraised = \"2026-08-18\"\ncontent = \"scratch fixture for 0199's PTY proof\"\n"
            ),
        )
        .unwrap();
    }
    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        "[tasks]\ndispatch-trait = \"demo\"\n",
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
    let path = ".ctx/traits/authored/demo/generated/index.toml";
    require_success(
        "approve fixture",
        &["traits", "trust", "approve", path],
        &repo,
        &home,
    );
    require_success(
        "activate fixture",
        &["traits", "activate", "demo"],
        &repo,
        &home,
    );
    Fixture {
        _scratch: scratch,
        repo,
        home,
    }
}

/// Every `RatatuiPane` inline construction issues crossterm's `ESC[6n`
/// cursor query as its first act; `expect`'s reply handler only ever fires
/// on a query that was actually sent. Two answered queries is therefore
/// direct proof of two distinct inline-pane constructions, independent of
/// screen content — the same signal the plan's Done-when condition names.
fn cursor_query_count(raw: &str) -> usize {
    raw.matches("\u{1b}[6n").count()
}

#[test]
fn task_queue_brings_up_the_inline_pane_for_every_member() {
    let fixture = failing_two_member_queue_fixture();
    let (exit_code, raw) = run_pty_with_cursor_reply(
        &ctx_bin(),
        "traits run --worktree --merge --task 0002 --task 0003 --continue-on-failure",
        &fixture.repo,
        &fixture.home,
        "__QUEUE_HANDOFF_COMPLETE__",
        ".ctx/queue-handoff-termios",
    );
    // `cmd = "false"` is a non-interactive command-permission rejection
    // (not a session failure), so both members complete without a merge
    // intent — this proof cares about the pane handoff, not the queue's
    // outcome classification, which `proof_task_queue_refusal_teardown.rs`
    // already covers.
    assert_eq!(
        exit_code, 0,
        "a rejected command step still lets the queue complete cleanly: {raw:?}"
    );

    assert!(
        !raw.contains("falling back to status progress"),
        "the cursor-position fallback fired — the pane teardown/construct handoff raced: {raw:?}"
    );
    assert!(
        cursor_query_count(&raw) >= 2,
        "expected two distinct inline-pane constructions (one cursor query each), got {}: {raw:?}",
        cursor_query_count(&raw)
    );

    let text = text_after_terminal_restore(&raw);
    let marker = text
        .find("__QUEUE_HANDOFF_COMPLETE__")
        .unwrap_or_else(|| panic!("post-exit completion marker was not visible: {text:?}"));
    let committed = &text[..marker];
    assert!(
        committed.contains("task queue:")
            && committed.contains("0002: completed")
            && committed.contains("0003: completed"),
        "task queue outcome table did not report both members: {committed:?}"
    );

    let termios = fs::read_to_string(fixture.repo.join(".ctx/queue-handoff-termios")).unwrap();
    let flags = termios.split_whitespace().collect::<Vec<_>>();
    assert!(
        flags.contains(&"icanon")
            && flags.contains(&"echo")
            && !flags.contains(&"-icanon")
            && !flags.contains(&"-echo"),
        "queue pane handoff left the slave terminal in raw mode: {termios:?}"
    );
}
