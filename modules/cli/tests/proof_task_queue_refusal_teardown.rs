//! PTY coverage for 0198: `--task` queue refusals and the outcome table
//! must survive the inline startup pane's teardown, exactly as a single
//! run's startup-stage failures already do (see
//! `proof_run_startup_progress.rs`). This file reuses that file's fixture,
//! stripping, and expect recipe verbatim rather than reimplementing it.

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

/// A `demo` trait with **no** `task-board` resource, plus a `runtime.toml`
/// that dispatches `--task` to it and a scratch task-board entry — the
/// live repro's shape: `--task 0003` cannot bind because the target trait
/// declares no board, so `resolve_dispatch_task` (`dispatch_preflight.rs`)
/// refuses before any session starts.
fn unbindable_dispatch_fixture() -> Fixture {
    let scratch = ScratchRoot::new("p0198-queue-refusal-teardown");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/authored/demo/generated")).unwrap();
    fs::create_dir_all(repo.join(".internal/tasks")).unwrap();
    git_init(&repo);
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/demo/generated/index.toml"),
        "id = \"demo\"\nschema-version = \"0.4\"\nversion = \"0.1.0\"\nname = \"Demo\"\ndescription = \"Demo trait that declares no task-board resource, so a --task dispatch cannot bind.\"\n\n[[port]]\nid = \"task\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Task to implement.\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[procedure]\ndescription = \"Run command\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"true\"\noutput = [\"slot:notified\"]\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    fs::write(
        repo.join(".internal/tasks/0003-demo.toml"),
        "schema-version = \"0.2\"\nkey = \"0003\"\ntitle = \"Scratch queue-refusal fixture\"\nstatus = \"ready\"\nraised = \"2026-08-18\"\ncontent = \"scratch fixture for 0198's PTY proof\"\n",
    )
    .unwrap();
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
        &["traits", "trust", "--approved", path],
        &repo,
        &home,
    );
    require_success(
        "activate fixture",
        &["traits", "internal", "state", "--active", "demo"],
        &repo,
        &home,
    );
    Fixture {
        _scratch: scratch,
        repo,
        home,
    }
}

/// A `demo` trait that DOES declare a `task-board` resource, dispatched to
/// by `runtime.toml`, plus a charter (`0001`) whose only child (`0001.1`)
/// is already `done` — `expand_task_queue` (`task_queue.rs`) filters closed
/// children out, so this charter expands to an empty queue without ever
/// calling `drive_session`. This is the second shape the live repro's fix
/// must cover: the per-member closure that used to be the only place
/// `startup.take()` fired never runs, so the pane must be torn down some
/// other way before `print_task_queue_report`.
fn empty_charter_fixture() -> Fixture {
    let scratch = ScratchRoot::new("p0198-empty-charter-teardown");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/authored/demo/generated")).unwrap();
    fs::create_dir_all(repo.join(".internal/tasks")).unwrap();
    git_init(&repo);
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/demo/generated/index.toml"),
        "id = \"demo\"\nschema-version = \"0.4\"\nversion = \"0.1.0\"\nname = \"Demo\"\ndescription = \"Demo trait with a task-board resource, for the empty-charter teardown proof.\"\n\n[[resource]]\nid = \"task-board\"\npath = \".internal/tasks\"\nroot = \"repo\"\ntrigger = \"on-demand\"\n\n[[port]]\nid = \"task\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Task to implement.\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[procedure]\ndescription = \"Run command\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"true\"\noutput = [\"slot:notified\"]\n",
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    fs::write(
        repo.join(".internal/tasks/0001-charter.toml"),
        "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"Scratch charter fixture\"\nraised = \"2026-08-18\"\ncontent = \"scratch charter for 0198's empty-queue PTY proof\"\n",
    )
    .unwrap();
    fs::write(
        repo.join(".internal/tasks/0001.1-child.toml"),
        "schema-version = \"0.2\"\nkey = \"0001.1\"\ntitle = \"Scratch closed child fixture\"\nstatus = \"done\"\nraised = \"2026-08-18\"\ncontent = \"scratch closed child for 0198's empty-queue PTY proof\"\n\n[relations]\nparent = \"0001\"\n",
    )
    .unwrap();
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
        &["traits", "trust", "--approved", path],
        &repo,
        &home,
    );
    require_success(
        "activate fixture",
        &["traits", "internal", "state", "--active", "demo"],
        &repo,
        &home,
    );
    Fixture {
        _scratch: scratch,
        repo,
        home,
    }
}

fn run_under_pty(fixture: &Fixture, args: &str, marker: &str, termios_file: &str) -> (i32, String) {
    run_pty_with_cursor_reply(
        &ctx_bin(),
        args,
        &fixture.repo,
        &fixture.home,
        marker,
        termios_file,
    )
}

#[test]
fn queue_dispatch_refusal_and_outcome_table_survive_startup_pane_teardown() {
    let fixture = unbindable_dispatch_fixture();
    let (exit_code, raw) = run_under_pty(
        &fixture,
        "traits run --worktree --merge --task 0003",
        "__QUEUE_REFUSAL_COMPLETE__",
        ".ctx/queue-refusal-termios",
    );
    assert_eq!(
        exit_code, 7,
        "expected the queue-halted exit code (app::error::EXIT_RUN_FAILED): {raw:?}"
    );
    let text = text_after_terminal_restore(&raw);
    let marker = text
        .find("__QUEUE_REFUSAL_COMPLETE__")
        .unwrap_or_else(|| panic!("post-exit completion marker was not visible: {text:?}"));
    let committed = &text[..marker];

    assert!(
        committed.contains("trait demo cannot bind task"),
        "cannot-bind refusal message did not survive teardown: {committed:?}"
    );
    assert!(
        committed.contains("ctx run --task 0003:"),
        "per-task refusal line did not survive teardown: {committed:?}"
    );
    assert!(
        committed.contains("task queue:") && committed.contains("0003: failed:"),
        "task queue outcome table did not survive teardown: {committed:?}"
    );
    assert!(
        committed.contains("halted — pass --continue-on-failure"),
        "halted hint did not survive teardown: {committed:?}"
    );

    let termios = fs::read_to_string(fixture.repo.join(".ctx/queue-refusal-termios")).unwrap();
    let flags = termios.split_whitespace().collect::<Vec<_>>();
    assert!(
        flags.contains(&"icanon")
            && flags.contains(&"echo")
            && !flags.contains(&"-icanon")
            && !flags.contains(&"-echo"),
        "queue refusal left the slave terminal in raw mode: {termios:?}"
    );
}

/// 0198 blocker `empty-queue-retains-startup-pane`: a charter whose only
/// child is already closed expands (`expand_task_queue`) to an empty
/// queue, so the per-member closure that normally consumes `startup`
/// never runs. `handle_task_queue_run` must still drop the pane before
/// `print_task_queue_report`'s plain-text rows — proven the same way as
/// the refusal case, by requiring the report to land after the pane's own
/// `Show` restore escape.
#[test]
fn empty_charter_queue_report_survives_startup_pane_teardown() {
    let fixture = empty_charter_fixture();
    let (exit_code, raw) = run_under_pty(
        &fixture,
        "traits run --worktree --merge --task 0001",
        "__EMPTY_CHARTER_COMPLETE__",
        ".ctx/empty-charter-termios",
    );
    assert_eq!(
        exit_code, 0,
        "an empty expanded queue has no halting outcome and should exit clean: {raw:?}"
    );
    let text = text_after_terminal_restore(&raw);
    let marker = text
        .find("__EMPTY_CHARTER_COMPLETE__")
        .unwrap_or_else(|| panic!("post-exit completion marker was not visible: {text:?}"));
    let committed = &text[..marker];

    assert!(
        committed.contains("task queue:"),
        "empty task queue report did not survive teardown: {committed:?}"
    );
    assert!(
        !committed.contains("halted"),
        "an empty, non-halting queue should not report a halt: {committed:?}"
    );

    let termios = fs::read_to_string(fixture.repo.join(".ctx/empty-charter-termios")).unwrap();
    let flags = termios.split_whitespace().collect::<Vec<_>>();
    assert!(
        flags.contains(&"icanon")
            && flags.contains(&"echo")
            && !flags.contains(&"-icanon")
            && !flags.contains(&"-echo"),
        "empty-charter queue report left the slave terminal in raw mode: {termios:?}"
    );
}
