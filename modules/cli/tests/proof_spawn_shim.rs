//! Behavioral proofs for the fork-free session-detach shim (2026-08-10):
//! `ctx __ctx-setsid-exec <argv…>` must detach into its own session, exec the
//! real argv with stdio/exit-code passthrough, and fail loudly — never
//! silently attached — when the target cannot run. The shim exists because
//! `pre_exec(setsid)` forced fork()+exec() and forking the multithreaded
//! `ctx` crashed spawned children on macOS
//! (`_os_once_gate_corruption_abort` in the atfork handlers, observed as
//! `git failed: git rev-parse --show-toplevel (no exit code)`).

use std::path::Path;

use support::{ScratchRoot, controlled_command, ctx_bin, utf8};

#[cfg(unix)]
#[test]
fn shim_execs_the_target_with_stdio_and_exit_code_passthrough() {
    let scratch = ScratchRoot::new("spawn-shim-passthrough");
    let home = scratch.home();
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).expect("create scratch cwd");

    let ctx = ctx_bin();
    let output = controlled_command(
        Path::new(&ctx),
        &[
            "__ctx-setsid-exec",
            "sh",
            "-c",
            "echo shim-stdout; echo shim-stderr >&2; exit 7",
        ],
        &repo,
        &home,
    )
    .output()
    .expect("shim invocation spawns");
    let (stdout, stderr) = utf8(&output);
    assert_eq!(
        output.status.code(),
        Some(7),
        "the target's exit code must pass through the exec: {stdout}{stderr}"
    );
    assert!(
        stdout.contains("shim-stdout"),
        "target stdout must pass through: {stdout}"
    );
    assert!(
        stderr.contains("shim-stderr"),
        "target stderr must pass through: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn shim_detaches_the_target_into_its_own_session() {
    let scratch = ScratchRoot::new("spawn-shim-session");
    let home = scratch.home();
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).expect("create scratch cwd");

    let ctx = ctx_bin();
    // After `setsid` the child is a fresh session-and-group leader, so its
    // pgid equals its own pid; a plain spawn would inherit this test's pgid
    // instead. (`ps -o sess=` is useless here: macOS prints a kernel struct
    // address — 0 — for every process.) This is also the exact contract
    // `run_kill`'s `kill(-pgid, …)` depends on.
    let output = controlled_command(
        Path::new(&ctx),
        &[
            "__ctx-setsid-exec",
            "sh",
            "-c",
            "echo $$; ps -o pgid= -p $$",
        ],
        &repo,
        &home,
    )
    .output()
    .expect("shim invocation spawns");
    let (stdout, stderr) = utf8(&output);
    assert!(output.status.success(), "pgid probe must succeed: {stderr}");
    let mut lines = stdout.lines();
    let child_pid = lines.next().unwrap_or_default().trim().to_string();
    let child_pgid = lines.next().unwrap_or_default().trim().to_string();
    assert!(
        !child_pid.is_empty() && child_pid == child_pgid,
        "the shim's target must lead its own process group (pid={child_pid:?}, pgid={child_pgid:?}):\n{stdout}"
    );
    let own_pgid = unsafe { libc::getpgid(0) };
    assert_ne!(
        child_pgid,
        own_pgid.to_string(),
        "the target's group must not be this test's group"
    );
}

#[cfg(unix)]
#[test]
fn shim_fails_loudly_when_the_target_cannot_run() {
    let scratch = ScratchRoot::new("spawn-shim-failures");
    let home = scratch.home();
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).expect("create scratch cwd");

    let ctx = ctx_bin();
    let missing_target = controlled_command(
        Path::new(&ctx),
        &["__ctx-setsid-exec", "definitely-not-a-real-program-xyz"],
        &repo,
        &home,
    )
    .output()
    .expect("shim invocation spawns");
    let (_, stderr) = utf8(&missing_target);
    assert_eq!(
        missing_target.status.code(),
        Some(127),
        "an unexecutable target must exit 127: {stderr}"
    );
    assert!(
        stderr.contains("cannot execute"),
        "the failure must name the condition: {stderr}"
    );

    let no_target = controlled_command(Path::new(&ctx), &["__ctx-setsid-exec"], &repo, &home)
        .output()
        .expect("shim invocation spawns");
    let (_, stderr) = utf8(&no_target);
    assert_eq!(
        no_target.status.code(),
        Some(2),
        "a sentinel with no target argv must exit 2: {stderr}"
    );
}
