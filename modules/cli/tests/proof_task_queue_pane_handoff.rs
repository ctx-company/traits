//! PTY coverage for 0199: a `--task` queue run must bring up the alternate-screen
//! run pane for EVERY member, never falling back to status progress while a
//! previous member's (or the startup pane's) input pump is still draining.
//! Reuses the expect/PTY recipe and fixture
//! shape `proof_task_queue_refusal_teardown.rs` already established for the
//! `--task` queue path (0198), rather than reimplementing it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use support::{
    ScratchRoot, ctx_bin, git_init, painted_pattern, require_success, run_pty_keys_after_markers,
    text_after_terminal_restore,
};

struct Fixture {
    _scratch: ScratchRoot,
    repo: std::path::PathBuf,
    home: std::path::PathBuf,
}

/// A `demo` trait that declares a `task-board` resource and a single
/// agent step, dispatched to by `runtime.toml`, plus two
/// independent `ready` tasks. Each queue member's fixture child emits a
/// marker then returns an incomplete result, so `create_run_panel`
/// (`drive.rs`) still builds a fresh run pane per member first, which is
/// exactly the handoff window 0199 closes.
fn failing_two_member_queue_fixture() -> Fixture {
    let scratch = ScratchRoot::new("p0199-queue-pane-handoff");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/authored/demo/generated")).unwrap();
    fs::create_dir_all(repo.join(".internal/tasks")).unwrap();
    git_init(&repo);
    let harness = home.join("ctx-fixture-queue-agent.sh");
    fs::write(
        &harness,
        "#!/bin/sh\nif [ \"$1\" = \"--fixture-probe\" ]; then\n  printf 'fixture-1.0\\n'\n  exit 0\nfi\ncount_file=\"$0.count\"\ncount=0\nif [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"$count_file\"\ncat >/dev/null\nprintf 'ctx-fixture-queue-child-%s\\n' \"$count\"\nprintf '{\"type\":\"result\",\"session_id\":\"fixture\",\"result\":\"{}\"}\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&harness, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/demo/generated/index.toml"),
        "id = \"demo\"\nschema-version = \"0.4\"\nversion = \"0.1.0\"\nname = \"Demo\"\ndescription = \"Demo trait with a task-board resource and a child-backed failing step.\"\n\n[[resource]]\nid = \"task-board\"\npath = \".internal/tasks\"\nroot = \"repo\"\ntrigger = \"on-demand\"\n\n[[port]]\nid = \"task\"\ndirection = \"input\"\nschema = \"schema:text\"\ndescription = \"Task to implement.\"\n\n[[agent]]\nid = \"worker\"\ndescription = \"Fixture worker\"\nsummary = \"Fixture worker\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[procedure]\ndescription = \"Run agent\"\n\n[[procedure.sequence]]\nid = \"agent\"\ntitle = \"Run agent\"\nagent = \"agent:worker\"\nprompt = \"Fail after spawning.\"\noutput = [\"slot:notified\"]\n",
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
        format!("[tasks]\ndispatch-trait = \"demo\"\n\n[harness.fixture]\nkind = \"custom\"\nbin = {:?}\ntransports = [\"cli\"]\nversion-probe = [\"--fixture-probe\"]\n\n[harness.fixture.cli]\nargv = []\nprompt-via = \"stdin\"\noutput = \"claude-stream-json\"\n\n[agent.role.worker]\nharness = \"fixture\"\ntransport = \"cli\"\n", harness.display().to_string()),
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
        &["traits", "state", "--active", "demo"],
        &repo,
        &home,
    );
    Fixture {
        _scratch: scratch,
        repo,
        home,
    }
}

/// Each run pane enters the alternate screen exactly once, making this a
/// construction signal independent of screen content.
fn alternate_screen_entries(raw: &str) -> usize {
    raw.matches("\u{1b}[?1049h").count()
}

fn shell_quote(value: &std::path::Path) -> String {
    format!(
        "'{}'",
        value.display().to_string().replace('\'', "'\\\"'\\\"'")
    )
}

#[test]
fn task_queue_brings_up_the_live_pane_for_every_member() {
    let fixture = failing_two_member_queue_fixture();
    let termios_file = fixture.repo.join(".ctx/queue-handoff-termios");
    let wrapper = fixture.repo.join(".ctx/queue-handoff-wrapper.sh");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n{} \"$@\"\nstatus=$?\nstty -a > {}\nexit $status\n",
            shell_quote(&ctx_bin()),
            shell_quote(&termios_file),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let first_failure_modal = painted_pattern("Resume/Retry");
    // This is emitted by the second member's spawned fixture child, after the
    // first Abort has released its modal.
    let second_member = painted_pattern("ctx-fixture-queue-child-2");
    let second_failure_modal = painted_pattern("Resume/Retry");
    let (exit_code, raw) = run_pty_keys_after_markers(
        &wrapper,
        "traits run --worktree --merge --max-retries 0 --task 0002 --task 0003 --continue-on-failure",
        &fixture.repo,
        &fixture.home,
        &[
            (first_failure_modal.as_str(), "\u{1b}[C\u{1b}[C\r"),
            (second_member.as_str(), ""),
            (second_failure_modal.as_str(), "\u{1b}[C\u{1b}[C\r"),
        ],
    );
    // `cmd = "false"` is a command-permission rejection, so neither member
    // completes and the queue halts — `EXIT_RUN_FAILED`, the same code
    // `proof_task_queue_refusal_teardown.rs` pins for this shape and the
    // behaviour 0215 introduced ("a failed run must report as failed").
    //
    // This proof does not own that classification; it is asserted only so a
    // change to it is noticed here rather than read as a pane regression.
    // What this proof owns is below: that the live pane came up for every
    // member and the teardown/construct handoff did not race.
    assert_eq!(
        exit_code, 7,
        "expected the queue-halted exit code (app::error::EXIT_RUN_FAILED): {raw:?}"
    );

    assert!(
        !raw.contains("falling back to status progress"),
        "the cursor-position fallback fired — the pane teardown/construct handoff raced: {raw:?}"
    );
    assert!(
        alternate_screen_entries(&raw) >= 2,
        "expected two distinct alternate-screen pane constructions, got {}: {raw:?}",
        alternate_screen_entries(&raw)
    );

    let text = text_after_terminal_restore(&raw);
    let committed = &text;
    // Both members reach the panel — the point of `--continue-on-failure`,
    // and the thing a pane that failed to hand off would cut short. The
    // outcome WORD is deliberately not asserted: it is classification, which
    // `proof_task_queue_refusal_teardown.rs` owns, and pinning it here made
    // this proof fail for a reason that had nothing to do with panes.
    assert!(
        committed.contains("task queue")
            && committed.contains("0002: ")
            && committed.contains("0003: "),
        "task queue panel did not report both members: {committed:?}"
    );

    let termios = fs::read_to_string(termios_file).unwrap();
    let flags = termios.split_whitespace().collect::<Vec<_>>();
    assert!(
        flags.contains(&"icanon")
            && flags.contains(&"echo")
            && !flags.contains(&"-icanon")
            && !flags.contains(&"-echo"),
        "queue pane handoff left the slave terminal in raw mode: {termios:?}"
    );
}

/// Without `--continue-on-failure` the queue halts on the first member, so
/// only one live pane and one failure modal ever come up — the zero- vs.
/// nonzero-remaining boundary the panel's `remaining`/`next` rows exist for.
#[test]
fn task_queue_names_remaining_work_when_it_halts_without_continue_on_failure() {
    let fixture = failing_two_member_queue_fixture();
    let termios_file = fixture.repo.join(".ctx/queue-halt-termios");
    let wrapper = fixture.repo.join(".ctx/queue-halt-wrapper.sh");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n{} \"$@\"\nstatus=$?\nstty -a > {}\nexit $status\n",
            shell_quote(&ctx_bin()),
            shell_quote(&termios_file),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let failure_modal = painted_pattern("Resume/Retry");
    let (exit_code, raw) = run_pty_keys_after_markers(
        &wrapper,
        "traits run --worktree --merge --max-retries 0 --task 0002 --task 0003",
        &fixture.repo,
        &fixture.home,
        &[(failure_modal.as_str(), "\u{1b}[C\u{1b}[C\r")],
    );
    assert_eq!(
        exit_code, 7,
        "expected the queue-halted exit code (app::error::EXIT_RUN_FAILED): {raw:?}"
    );
    assert!(
        !raw.contains("falling back to status progress"),
        "the cursor-position fallback fired: {raw:?}"
    );
    assert_eq!(
        alternate_screen_entries(&raw),
        1,
        "only the first member should ever bring up a live pane, got {}: {raw:?}",
        alternate_screen_entries(&raw)
    );

    let committed = text_after_terminal_restore(&raw);
    assert!(
        committed.contains("0002: "),
        "the attempted member's row did not survive: {committed:?}"
    );
    assert!(
        !committed.contains("0003"),
        "the never-attempted member must not get a row: {committed:?}"
    );
    assert!(
        committed.contains("remaining") && committed.contains('1'),
        "the panel must name exactly one member left unattempted: {committed:?}"
    );
    assert!(
        committed.contains("--continue-on-failure"),
        "the panel must name the flag that resumes the rest: {committed:?}"
    );
    assert!(
        committed.contains("Failure"),
        "a halted queue must close as Failure: {committed:?}"
    );

    let termios = fs::read_to_string(termios_file).unwrap();
    let flags = termios.split_whitespace().collect::<Vec<_>>();
    assert!(
        flags.contains(&"icanon")
            && flags.contains(&"echo")
            && !flags.contains(&"-icanon")
            && !flags.contains(&"-echo"),
        "queue halt teardown left the slave terminal in raw mode: {termios:?}"
    );
}
