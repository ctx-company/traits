//! PTY smoke coverage for startup terminal fallback.

use std::fs;
use std::process::Command;

use std::path::PathBuf;

use support::{
    ScratchRoot, ctx_bin, git_init, require_success, run_pty_with_cursor_reply, strip_escapes,
};

struct Fixture {
    _scratch: ScratchRoot,
    repo: PathBuf,
    home: PathBuf,
}

fn command_trait_fixture() -> Fixture {
    let scratch = ScratchRoot::new("p551-startup-before-config");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init(&repo);
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        "id = \"demo\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"Demo\"\nsummary = \"Demo\"\n\n[procedure]\ndescription = \"Run command\"\n\n[[slot]]\nid = \"notified\"\nschema = \"schema:text\"\n\n[[procedure.sequence]]\nid = \"command\"\ntitle = \"Run command\"\nkind = \"command\"\ncmd = \"true\"\noutput = [\"slot:notified\"]\n",
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
    let path = ".ctx/traits/demo/generated/index.toml";
    require_success(
        "approve fixture",
        &["traits", "trust", "--approved", path],
        &repo,
        &home,
    );
    require_success(
        "activate fixture",
        &["traits", "state", "--active", "--file", path],
        &repo,
        &home,
    );
    Fixture {
        _scratch: scratch,
        repo,
        home,
    }
}

fn untrusted_query_fixture() -> Fixture {
    const SENTINEL: &str = "PREAUTH_STARTUP_SENTINEL";
    let scratch = ScratchRoot::new("p551-startup-preauthorization");
    let home = scratch.home();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(format!(".ctx/traits/{SENTINEL}/generated"))).unwrap();
    git_init(&repo);
    let path = format!(".ctx/traits/{SENTINEL}/generated/index.toml");
    fs::write(
        repo.join(&path),
        format!(
            "id = \"{SENTINEL}\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"benign query fixture\"\nsummary = \"benign {SENTINEL}\"\nstatus = \"draft\"\n"
        ),
    )
    .unwrap();
    fs::write(
        repo.join(format!(".ctx/traits/{SENTINEL}/trait.toml")),
        format!(
            "[package]\nid = \"{SENTINEL}\"\nversion = \"0.1.0\"\nname = \"benign query fixture\"\nstatus = \"active\"\n"
        ),
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
    Fixture {
        _scratch: scratch,
        repo,
        home,
    }
}

fn make_delayed_config_fifo(fixture: &Fixture) {
    let status = Command::new("mkfifo")
        .arg(".ctx/traits/runtime.toml")
        .current_dir(&fixture.repo)
        .status()
        .unwrap();
    assert!(status.success(), "could not create delayed config FIFO");
}

/// True once the startup pane's title reached the PTY — whether or not the
/// cell diff skipped the space between its words (see [`strip_escapes`]).
fn saw_startup_pane(raw: &str) -> bool {
    let text = strip_escapes(raw);
    text.contains("Run startup") || text.contains("Runstartup")
}

fn failed_startup_pty(
    fixture: &Fixture,
    command: &str,
    label: &str,
    reason: &str,
    check_termios: bool,
) {
    let output = Command::new("expect")
        .args([
            "-c",
            r#"
                set timeout 30
                set child_status {}
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; $env(CTX_STARTUP_COMMAND); status=\$?; stty -a > .ctx/startup-failure-termios; printf '__STARTUP_FAILURE_COMPLETE__\\n'; exit \$status"
                expect {
                    -re {\x1b\[6n} { send -- "\033\[40;120R"; exp_continue }
                    eof { set child_status [wait] }
                }
                if {[lindex $child_status 3] == 0} { exit 1 }
            "#,
        ])
        .current_dir(&fixture.repo)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &fixture.home)
        .env("XDG_CACHE_HOME", &fixture.home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("TERM", "xterm-256color")
        .env("CTX_STARTUP_BIN", ctx_bin())
        .env("CTX_STARTUP_COMMAND", command)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "failed startup PTY failed: {output:?}"
    );
    let output = String::from_utf8_lossy(&output.stdout);
    let marker = output
        .find("__STARTUP_FAILURE_COMPLETE__")
        .unwrap_or_else(|| panic!("post-exit failure marker was not visible: {output:?}"));
    let committed = &output[..marker];
    assert!(
        committed.contains(label),
        "failed startup did not preserve {label:?} in scrollback: {output:?}"
    );
    assert!(
        committed.contains(reason),
        "failed startup did not preserve {reason:?} in scrollback: {output:?}"
    );
    if check_termios {
        let termios =
            fs::read_to_string(fixture.repo.join(".ctx/startup-failure-termios")).unwrap();
        let flags = termios.split_whitespace().collect::<Vec<_>>();
        assert!(
            flags.contains(&"icanon")
                && flags.contains(&"echo")
                && !flags.contains(&"-icanon")
                && !flags.contains(&"-echo"),
            "failed startup left the slave terminal in raw mode: {termios:?}"
        );
    }
}

#[test]
fn startup_pty_commits_each_failed_stage_before_restoring_the_terminal() {
    let fixture = command_trait_fixture();
    failed_startup_pty(
        &fixture,
        "$CTX_STARTUP_BIN traits run --file missing.toml",
        "Initialization",
        "trait authorization was refused",
        true,
    );

    let fixture = untrusted_query_fixture();
    failed_startup_pty(
        &fixture,
        "$CTX_STARTUP_BIN traits run -- benign",
        "Trust gate",
        "trait authorization was refused",
        false,
    );

    let fixture = command_trait_fixture();
    failed_startup_pty(
        &fixture,
        "$CTX_STARTUP_BIN traits run --file .ctx/traits/demo/generated/index.toml -- --unknown=value",
        "Harness probe",
        "unknown trait argument",
        false,
    );

    let fixture = command_trait_fixture();
    failed_startup_pty(
        &fixture,
        "$CTX_STARTUP_BIN traits run --file .ctx/traits/demo/generated/index.toml --worktree=../invalid",
        "Worktree",
        "worktree id",
        false,
    );

    let fixture = command_trait_fixture();
    fs::write(
        fixture.repo.join(".ctx/traits/runtime.toml"),
        "[worktree]\nseed = [\"../invalid\"]\n",
    )
    .unwrap();
    failed_startup_pty(
        &fixture,
        "$CTX_STARTUP_BIN traits run --file .ctx/traits/demo/generated/index.toml --worktree=seed-failure",
        "Seeding",
        "worktree seed",
        false,
    );

    let fixture = command_trait_fixture();
    fs::write(
        fixture.repo.join(".ctx/traits/runtime.toml"),
        "[worktree]\nwarm = [\"../invalid\"]\n",
    )
    .unwrap();
    failed_startup_pty(
        &fixture,
        "$CTX_STARTUP_BIN traits run --file .ctx/traits/demo/generated/index.toml --worktree=warm-failure",
        "Warm",
        "worktree warm path",
        false,
    );
}

#[test]
fn startup_pty_falls_back_to_the_existing_initialization_line_when_unavailable() {
    let fixture = command_trait_fixture();

    // A PTY that allocates but never answers ratatui's cursor query, which is
    // what exercises the allocation-error fallback without corrupting the
    // normal line-oriented startup narration. `expect` with no reply handler
    // is exactly that, and it is the same tool the other proofs in this file
    // already use. `script` was doing this job and could not: its argument
    // grammar differs between BSD and util-linux — `script -q /dev/null CMD`
    // runs the command on macOS and is rejected on Linux with "unrecognized
    // option", so this proof passed locally and failed on every Linux runner.
    let output = Command::new("expect")
        .args([
            "-c",
            r#"
                set timeout 30
                spawn -noecho $env(CTX_STARTUP_BIN) traits run --file .ctx/traits/demo/generated/index.toml
                expect eof
            "#,
        ])
        .current_dir(&fixture.repo)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &fixture.home)
        .env("XDG_CACHE_HOME", &fixture.home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("CTX_STARTUP_BIN", ctx_bin())
        .output()
        .unwrap();
    assert!(output.status.success(), "startup PTY failed: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("ctx run · initialization"),
        "unavailable startup pane did not preserve line narration: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn startup_pty_uses_the_inline_pane_when_the_pty_has_a_size() {
    let fixture = command_trait_fixture();
    // `script` deliberately leaves cursor-position queries unanswered. Expect
    // provides the missing terminal reply so this covers the successful inline
    // owner path rather than the allocation-fallback case above.
    let (exit_code, output) = run_pty_with_cursor_reply(
        &ctx_bin(),
        "traits run --file .ctx/traits/demo/generated/index.toml",
        &fixture.repo,
        &fixture.home,
        "__STARTUP_HANDOFF_COMPLETE__",
        ".ctx/startup-success-termios",
    );
    assert_eq!(exit_code, 0, "startup PTY failed: {output:?}");
    assert!(
        saw_startup_pane(&output),
        "sized PTY did not allocate the startup pane: {output:?}"
    );
    assert!(
        !output.contains("ctx run · initialization"),
        "startup pane unexpectedly fell back to line narration: {output:?}"
    );
    assert!(
        output.contains("information"),
        "startup pane did not hand off into the live run layout: {output:?}"
    );
    assert!(
        output.contains("__STARTUP_HANDOFF_COMPLETE__"),
        "post-exit shell marker was not visible after startup handoff: {output:?}"
    );
    let visible = strip_escapes(&output);
    assert!(
        visible
            .find("Runstartup")
            .or_else(|| visible.find("Run startup"))
            < visible.find("information"),
        "startup content did not precede the live run content: {output:?}"
    );
    let termios = fs::read_to_string(fixture.repo.join(".ctx/startup-success-termios")).unwrap();
    let flags = termios.split_whitespace().collect::<Vec<_>>();
    assert!(
        flags.contains(&"icanon")
            && flags.contains(&"echo")
            && !flags.contains(&"-icanon")
            && !flags.contains(&"-echo"),
        "successful handoff left the slave terminal in raw mode: {termios:?}"
    );
}

#[test]
fn startup_pty_redraws_after_resize_while_configuration_is_delayed() {
    let fixture = command_trait_fixture();
    make_delayed_config_fifo(&fixture);

    let output = Command::new("expect")
        .args([
            "-c",
            r#"
                set timeout 3
                set startup_frames 0
                set child_status {}
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; exec $env(CTX_STARTUP_BIN) traits run --file .ctx/traits/demo/generated/index.toml"
                expect {
                    -re {\x1b\[6n} { send -- "\033\[40;120R"; exp_continue }
                    -re {Run(\x1b\[[0-9;]*H)? ?startup} {
                        incr startup_frames
                        if {$startup_frames == 1} {
                            stty rows 30 columns 90 < $spawn_id
                            exp_continue
                        }
                        if {$startup_frames == 2} {
                            send -- "\003"
                            exp_continue
                        }
                    }
                    eof { set child_status [wait] }
                    timeout { exit 1 }
                }
                if {$startup_frames < 2 || [lindex $child_status 3] != 130} { exit 1 }
            "#,
        ])
        .current_dir(&fixture.repo)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &fixture.home)
        .env("XDG_CACHE_HOME", &fixture.home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("TERM", "xterm-256color")
        .env("CTX_STARTUP_BIN", ctx_bin())
        .output()
        .unwrap();
    assert!(output.status.success(), "resize PTY failed: {output:?}");
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(
        saw_startup_pane(&output),
        "resize did not render startup: {output:?}"
    );
}

#[test]
fn startup_pty_uses_status_narration_when_live_tui_is_environment_ineligible() {
    let fixture = command_trait_fixture();

    for (name, value) in [("CI", "1"), ("NO_COLOR", "1"), ("TERM", "dumb")] {
        let output = Command::new("expect")
            .args([
                "-c",
                r#"
                    set timeout 30
                    spawn -noecho /bin/sh -c "stty cols 120 rows 40; exec $env(CTX_STARTUP_BIN) traits run --progress tui --file .ctx/traits/demo/generated/index.toml"
                    expect eof
                "#,
            ])
            .current_dir(&fixture.repo)
            .env_clear()
            .env("HOME", &fixture.home)
            .env("XDG_CONFIG_HOME", &fixture.home)
            .env("XDG_CACHE_HOME", &fixture.home)
            .env("PATH", std::env::var("PATH").unwrap())
            .env("TERM", "xterm-256color")
            .env(name, value)
            .env("CTX_STARTUP_BIN", ctx_bin())
            .output()
            .unwrap();
        assert!(output.status.success(), "{name} PTY failed: {output:?}");
        let output = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.contains("ctx run · initialization"),
            "{name} did not retain status initialization narration: {output:?}"
        );
        assert!(
            !saw_startup_pane(&output),
            "{name} unexpectedly allocated the startup pane: {output:?}"
        );
    }
}

#[test]
fn startup_pty_is_visible_before_delayed_config_resolution() {
    let fixture = command_trait_fixture();
    make_delayed_config_fifo(&fixture);

    // The FIFO holds `resolve_runtime_config` after startup construction. The
    // expect branch can only see this title before it starts the writer, which
    // makes the ordering assertion independent of a fast local filesystem.
    let output = Command::new("expect")
        .args([
            "-c",
            r#"
                set timeout 3
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; exec $env(CTX_STARTUP_BIN) traits run --file .ctx/traits/demo/generated/index.toml"
                expect {
                    -re {\x1b\[6n} { send -- "\033\[40;120R"; exp_continue }
                    -re {Run(\x1b\[[0-9;]*H)? ?startup} {
                        exit 0
                    }
                    timeout { exit 1 }
                }
            "#,
        ])
        .current_dir(&fixture.repo)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &fixture.home)
        .env("XDG_CACHE_HOME", &fixture.home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("TERM", "xterm-256color")
        .env("CTX_STARTUP_BIN", ctx_bin())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "delayed startup PTY failed: {output:?}"
    );
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(
        saw_startup_pane(&output),
        "startup pane was not visible while delayed configuration was pending: {output:?}"
    );
}

#[test]
fn startup_pty_ctrl_c_preserves_interrupted_scrollback_before_live_handoff() {
    let fixture = command_trait_fixture();
    make_delayed_config_fifo(&fixture);

    let output = Command::new("expect")
        .args([
            "-c",
            r#"
                set timeout 3
                set interrupted 0
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; $env(CTX_STARTUP_BIN) traits run --file .ctx/traits/demo/generated/index.toml; status=\$?; stty -a > .ctx/startup-termios; exit \$status"
                expect {
                    -re {\x1b\[6n} { send -- "\033\[40;120R"; exp_continue }
                    -re {Run(\x1b\[[0-9;]*H)? ?startup} {
                        if {!$interrupted} {
                            set interrupted 1
                            send -- "\003"
                            exp_continue
                        }
                    }
                    eof { set child_status [wait] }
                    timeout { exit 1 }
                }
                if {!$interrupted} { exit 1 }
                if {[lindex $child_status 3] != 130} { exit 1 }
            "#,
        ])
        .current_dir(&fixture.repo)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &fixture.home)
        .env("XDG_CACHE_HOME", &fixture.home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("TERM", "xterm-256color")
        .env("CTX_STARTUP_BIN", ctx_bin())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Ctrl-C did not terminate the startup child itself: {output:?}"
    );
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.contains("interrupted"),
        "interrupted startup rows were not committed to scrollback: {output:?}"
    );
    assert!(
        !output.contains("Run Panel"),
        "Ctrl-C unexpectedly reached a live run frame: {output:?}"
    );
    assert!(
        output.contains("run startup interrupted; terminal restored")
            && output.contains("\x1b[?25h"),
        "Ctrl-C did not restore the inline terminal to cooked, visible-cursor mode: {output:?}"
    );
    let termios = fs::read_to_string(fixture.repo.join(".ctx/startup-termios")).unwrap();
    // Every token, not the `lflags:` line: BSD `stty -a` labels its groups
    // (`lflags: icanon isig ... echo`, with tab-indented continuations) and
    // GNU `stty -a` prints the same flags with no label at all, so parsing
    // the label found nothing on Linux and this proof failed there alone.
    // `icanon`/`echo` are local flags in both formats and appear nowhere else
    // in the dump, so scanning the whole file is exact rather than merely
    // lenient — and `-echoctl`/`-echoprt` are distinct tokens from `-echo`.
    let flags = termios.split_whitespace().collect::<Vec<_>>();
    assert!(
        flags.contains(&"icanon") || flags.contains(&"-icanon"),
        "stty output did not contain local flags: {termios:?}"
    );
    assert!(
        flags.contains(&"icanon")
            && flags.contains(&"echo")
            && !flags.contains(&"-icanon")
            && !flags.contains(&"-echo"),
        "Ctrl-C left the slave terminal in raw mode: {termios:?}"
    );
}

#[test]
fn startup_pty_query_refusal_never_commits_untrusted_trait_details() {
    const SENTINEL: &str = "PREAUTH_STARTUP_SENTINEL";
    let fixture = untrusted_query_fixture();
    let output = Command::new("expect")
        .args([
            "-c",
            r#"
                set timeout 30
                set child_status {}
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; exec $env(CTX_STARTUP_BIN) traits run -- benign"
                expect {
                    -re {\x1b\[6n} { send -- "\033\[40;120R"; exp_continue }
                    eof { set child_status [wait] }
                }
                if {[lindex $child_status 3] == 0} { exit 1 }
            "#,
        ])
        .current_dir(&fixture.repo)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &fixture.home)
        .env("XDG_CACHE_HOME", &fixture.home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("TERM", "xterm-256color")
        .env("CTX_STARTUP_BIN", ctx_bin())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "untrusted query PTY failed: {output:?}"
    );
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.contains("trait authorization was refused"),
        "startup refusal was not generic: {output:?}"
    );
    assert!(
        !output.contains(SENTINEL),
        "untrusted trait detail leaked into PTY redraw or scrollback: {output:?}"
    );
}
