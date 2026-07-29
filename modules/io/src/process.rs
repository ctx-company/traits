//! Process launch primitives for the P423 dashboard: detaching a new
//! `ctx traits run` child from the current process/terminal without a
//! daemon, and invoking `$EDITOR` with inherited terminal IO for the
//! dashboard's editor-backed text operations (spawn request, trust
//! approve/block reason).
//!
//! Neither helper here decides *what* to run — argv construction/validation
//! (rejecting dashboard-breaking flags, injecting `--progress none`) is the
//! dashboard's own responsibility. This module only owns how the child is
//! detached and how the terminal is handed to/reclaimed from an editor.

use camino::Utf8Path;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

/// Spawn `argv[0]` with `argv[1..]` as a fully detached child: a new Unix
/// session (`setsid`) so it survives the parent/dashboard exiting and is not
/// delivered the parent's terminal signals (e.g. a `Ctrl-C` sent to the
/// dashboard's controlling terminal), running with `cwd` as its working
/// directory, and stdout/stderr redirected to `log_path` (truncated) so
/// output survives after the parent detaches instead of racing an
/// already-closed pipe. `envs` is additional environment set on the child
/// only (P510: the session-keyed log path, so the child driver can record
/// its own log path into the liveness index without a new CLI flag). Returns
/// the still-open `Child` purely so the caller may poll `try_wait`; dropping
/// it must never terminate the process — this module never calls `kill` on
/// it.
pub fn spawn_detached(
    exe: &Utf8Path,
    args: &[String],
    cwd: &Utf8Path,
    log_path: &Utf8Path,
    envs: &[(&str, &str)],
) -> crate::Result<Child> {
    let log_file = std::fs::File::create(log_path.as_std_path()).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: log_path.to_string(),
            source: e,
        }
    })?;
    let log_file_err = log_file
        .try_clone()
        .map_err(|e| crate::environment::Error::Filesystem {
            path: log_path.to_string(),
            source: e,
        })?;

    let mut command = Command::new(exe.as_std_path());
    command
        .args(args)
        .current_dir(cwd.as_std_path())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err));
    for (key, value) in envs {
        command.env(key, value);
    }

    // SAFETY: `setsid` is called in the forked child before `exec`, between
    // `fork` and `exec` where only async-signal-safe operations are
    // permitted; `libc::setsid` is async-signal-safe and touches no memory
    // shared with the parent. This detaches the child into its own session
    // (and therefore process group) so it has no controlling terminal and is
    // immune to signals delivered to the dashboard's terminal/process group.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    command
        .spawn()
        .map_err(|e| crate::environment::Error::Process {
            command: Some(exe.to_string()),
            path: Some(cwd.to_string()),
            exit_status: None,
            timed_out: false,
            message: format!("failed to spawn detached process: {e}"),
        })
        .map_err(Into::into)
}

/// Resolve `$EDITOR` (falling back to `vi`, matching common shell/git
/// convention) as a `sh -c` command line so a configured editor containing
/// its own arguments (e.g. `code --wait`) works without the dashboard
/// re-implementing shell word-splitting.
fn editor_command() -> String {
    std::env::var("EDITOR")
        .ok()
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// Run `$EDITOR` on `path` with the terminal's stdin/stdout/stderr inherited
/// directly (no capture), so the editor gets full interactive control of the
/// terminal. The caller must have already suspended any raw/alternate-screen
/// terminal state (e.g. via the dashboard's ratatui suspend scope) before
/// calling this, and must re-enter it afterward regardless of the outcome —
/// this function only reports the editor's exit status, it does not touch
/// terminal modes itself. `Ok(true)` means the editor exited 0; `Ok(false)`
/// means it exited nonzero (the caller decides whether that means "discard
/// the edit" or "keep whatever partial content is on disk").
pub fn run_editor_inherited(path: &Utf8Path) -> crate::Result<bool> {
    let editor = editor_command();
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\"",))
        .arg("--")
        .arg(path.as_str())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| crate::environment::Error::Process {
            command: Some(editor.clone()),
            path: Some(path.to_string()),
            exit_status: None,
            timed_out: false,
            message: format!("failed to launch editor: {e}"),
        })?;
    Ok(status.success())
}
