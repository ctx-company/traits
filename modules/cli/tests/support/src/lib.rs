//! Shared scaffolding for `ctx-traits-cli` integration tests (P461).
//!
//! Mirrors `scripts/byte_compare.rs`'s controlled-environment policy exactly:
//! ambient variables cleared, locale/color pinned, `HOME`/XDG/`TMPDIR`
//! isolated per test, and only the executable/toolchain paths a command
//! actually needs (`PATH`, `RUSTUP_HOME`, `CARGO_HOME`) preserved. No
//! provider or network environment is ever passed through.
//!
//! P477: this is its own crate (a `[dev-dependencies]` path dependency of
//! `ctx-traits-cli`, `modules/cli/tests/support`), not a `mod support;` each
//! `modules/cli/tests/*.rs` file used to include inline. A real lib crate's
//! `pub` API is not dead-code-linted per consumer the way an inlined module
//! was — a helper only some proof suites call no longer needs suppressing or
//! deleting to keep every individual test binary's compilation warning-free.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Absolute path to the `ctx` binary under test, built by Cargo before the
/// test binary runs. Read at runtime (not via the `env!` macro): `support`
/// is a separate crate from `ctx-traits-cli` (P477), so Cargo never
/// substitutes `CARGO_BIN_EXE_ctx` into ITS compilation the way it does for
/// `ctx-traits-cli`'s own integration-test binaries — but Cargo does set it
/// as a real process environment variable for those binaries at run time,
/// which this crate's code executes inside once linked in.
/// Strips ANSI escape sequences (CSI, OSC, and two-byte ESC forms) from a
/// raw PTY stream, leaving only the printable payload. The pane renderer's
/// cell diff may skip any cell whose content and style already match the
/// screen — a plain-styled space over blank ground — so adjacent words can
/// arrive split by a cursor move ("Run\x1b[1;6Hstartup"). PTY proofs run
/// text assertions against this stripped form, never against the raw byte
/// layout, which would freeze the diff accident of the moment (2026-08-04:
/// dropping BOLD from focused pane titles made the title's space cell
/// identical to blank ground and turned four of these proofs red).
pub fn strip_escapes(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            text.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for follow in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&follow) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(follow) = chars.next() {
                    if follow == '\u{7}' {
                        break;
                    }
                    if follow == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    text
}

pub fn ctx_bin() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_BIN_EXE_ctx")
            .expect("cargo test sets CARGO_BIN_EXE_ctx for ctx-traits-cli's integration tests"),
    )
}

/// Absolute path to the repository root, resolved at runtime (P477 rule,
/// task 0099): a baked `env!("CARGO_MANIFEST_DIR")` here would freeze
/// whatever worktree happened to compile this crate, which then survives as
/// a stale path inside a shared build-slot's cached `.rlib`/test binary long
/// after that worktree is pruned. Cargo sets `CARGO_MANIFEST_DIR` as a real
/// process env var at run time for the *package under test* linking this
/// crate in — `modules/cli`, not `modules/cli/tests/support` itself — so the
/// hop count from that runtime root is two levels, not the four a
/// compile-time bake from `support`'s own manifest dir would need. The
/// landmark probe (`Cargo.toml` + `modules/` at the candidate) turns a wrong
/// hop count into a loud panic naming the resolved path instead of a silent
/// bad root.
pub fn repo_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("cargo test sets CARGO_MANIFEST_DIR for ctx-traits-cli's integration tests");
    let candidate = PathBuf::from(manifest_dir)
        .parent()
        .and_then(Path::parent)
        .expect("modules/cli's CARGO_MANIFEST_DIR has a repo root two levels up")
        .to_path_buf();
    assert!(
        candidate.join("Cargo.toml").is_file() && candidate.join("modules").is_dir(),
        "repo_root() resolved {} but it has no Cargo.toml/modules landmark — the runtime \
         CARGO_MANIFEST_DIR hop count is wrong",
        candidate.display()
    );
    candidate
}

/// Absolute path to `ctx-fixture-agent` (P461), the dev-only multi-role
/// stub CLI harness built as a workspace sibling of `ctx` — resolved next
/// to [`ctx_bin`] rather than via a second `CARGO_BIN_EXE_*` env var, since
/// `support` (unlike `ctx-traits-cli`'s own test binaries) is never
/// Cargo-substituted for a crate it is not itself a dependency of.
pub fn fixture_agent_bin() -> PathBuf {
    ctx_bin()
        .parent()
        .expect("ctx binary path has a parent directory")
        .join("ctx-fixture-agent")
}

/// Symlinks the repository's own already-installed `node_modules` into a
/// scratch project, so a `config.ts` build resolves `@ctx-traits/config`
/// (and any other repo-local package) without a network fetch or a second
/// install — the one local dependency `ctx traits config build` needs on
/// its node-invoking path (P457). Shared by every suite that drives a real
/// `config.ts` build (P467's honesty layer, P457's own proofs).
pub fn symlink_node_modules(proj: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(repo_root().join("node_modules"), proj.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(repo_root().join("node_modules"), proj.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));
}

/// A private, per-test scratch root under the OS temp directory, removed on
/// drop. Never shared across tests: each caller gets its own directory named
/// with this process's pid and a process-local monotonic counter, so
/// parallel test threads (and parallel `cargo test` invocations, e.g. old
/// harness + new suite running side by side during migration) never collide.
pub struct ScratchRoot {
    dir: PathBuf,
}

impl ScratchRoot {
    /// Create a fresh, empty scratch root under `label` (e.g. a test name).
    ///
    /// Uses `fs::create_dir` (not `create_dir_all`), which fails if the leaf
    /// directory already exists, so a stale directory left behind by an
    /// interrupted prior process (possible under PID reuse, since the name
    /// is keyed on `process::id()`) is never silently reused as this test's
    /// scratch root. On a collision the counter is advanced and another
    /// candidate name is tried, retrying until an actually-fresh directory
    /// is created.
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir();
        loop {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!(
                "ctx-traits-cli-test-{}-{unique}-{label}",
                std::process::id()
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Self { dir },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    panic!("cannot create scratch root {}: {error}", dir.display())
                }
            }
        }
    }

    /// This scratch root's own directory, for callers that need a fresh
    /// sub-path inside it rather than just the dedicated `home()`
    /// subdirectory.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// A dedicated `HOME` subdirectory, isolating `~/.config/ctx/trust.toml`
    /// and any other real machine state from this test.
    pub fn home(&self) -> PathBuf {
        let home = self.dir.join("home");
        std::fs::create_dir_all(&home).unwrap_or_else(|error| {
            panic!("cannot create scratch home {}: {error}", home.display())
        });
        home
    }
}

impl Drop for ScratchRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Build a controlled-environment `Command` for `binary`/`args`/`cwd`/`home`,
/// identical in policy to `scripts/byte_compare.rs`'s `controlled_command`:
/// ambient environment cleared; `LC_ALL`/`LANG` pinned to `C`; `NO_COLOR=1`;
/// `HOME`/`XDG_CONFIG_HOME`/`XDG_CACHE_HOME`/`TMPDIR` rooted at `home`; only
/// `PATH`, `RUSTUP_HOME`, and `CARGO_HOME` preserved from the real
/// environment (needed by `ctx traits merge`'s landing-gate `rustc`/`cargo`
/// shims and `ctx traits build`'s `node` shell-out); no provider/network
/// credentials ever passed through.
pub fn controlled_command(binary: &Path, args: &[&str], cwd: &Path, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_CACHE_HOME", home)
        .env("TMPDIR", home)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Ok(path) = std::env::var("PATH") {
        command.env("PATH", path);
    }
    if let Ok(rustup_home) = std::env::var("RUSTUP_HOME") {
        command.env("RUSTUP_HOME", rustup_home);
    } else if let Ok(real_home) = std::env::var("HOME") {
        command.env("RUSTUP_HOME", Path::new(&real_home).join(".rustup"));
    }
    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        command.env("CARGO_HOME", cargo_home);
    } else if let Ok(real_home) = std::env::var("HOME") {
        command.env("CARGO_HOME", Path::new(&real_home).join(".cargo"));
    }
    command
}

/// Run `ctx` with `args` in `cwd`, isolated under `home`. Panics only on
/// spawn failure (e.g. missing binary) — exit code and output are left for
/// the caller to assert.
pub fn run_ctx(args: &[&str], cwd: &Path, home: &Path) -> Output {
    controlled_command(&ctx_bin(), args, cwd, home)
        .output()
        .unwrap_or_else(|error| panic!("cannot execute {}: {error}", ctx_bin().display()))
}

/// Spawn `ctx` with `args` in `cwd`, isolated under `home`, without waiting
/// for it to exit — the controlled-environment counterpart to [`run_ctx`]
/// for a proof that must observe the child mid-flight (e.g. its pid, or a
/// marker file it writes before it finishes) rather than only its final
/// output. Stdout/stderr stay piped so a caller can still collect them via
/// `wait_with_output` once the child is done. Panics only on spawn failure.
pub fn spawn_ctx(args: &[&str], cwd: &Path, home: &Path) -> std::process::Child {
    controlled_command(&ctx_bin(), args, cwd, home)
        .spawn()
        .unwrap_or_else(|error| panic!("cannot spawn {}: {error}", ctx_bin().display()))
}

/// Run `git` with `args` in `cwd`, under the same controlled environment as
/// `ctx` invocations (`env_clear`, `LC_ALL`/`LANG=C`, `HOME`/`XDG_*`/`TMPDIR`
/// rooted at `home`) — so a proof's outcome (in particular, which files
/// `git add -A` stages) never depends on the invoking machine's personal
/// `~/.gitconfig`/`core.excludesFile`. Panics only on spawn failure; exit
/// code and output are left for the caller to assert.
pub fn run_git(args: &[&str], cwd: &Path, home: &Path) -> Output {
    controlled_command(Path::new("git"), args, cwd, home)
        .output()
        .unwrap_or_else(|error| panic!("cannot run git {args:?}: {error}"))
}

/// Like [`run_ctx`], but layering `extra_env` on top of the controlled
/// environment. `ctx`'s own harness dispatch never clears its process
/// environment before spawning a custom-kind harness child
/// (`modules/io/src/harness.rs`), so a `CTX_FIXTURE_*` variable set here on
/// the `ctx traits run` invocation reaches `ctx-fixture-agent` (P461)
/// unchanged — the mechanism `proof_park_honesty` uses to select a
/// reviewer's verdict without templating per-scenario argv into `ctx.toml`.
pub fn run_ctx_with_env(
    args: &[&str],
    cwd: &Path,
    home: &Path,
    extra_env: &[(&str, &str)],
) -> Output {
    let mut command = controlled_command(&ctx_bin(), args, cwd, home);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command
        .output()
        .unwrap_or_else(|error| panic!("cannot execute {}: {error}", ctx_bin().display()))
}

/// Run `ctx` with `args` in `cwd`, isolated under `home`, feeding `stdin`
/// bytes to the child's stdin (closed after the write, so the child sees
/// EOF). A sibling of [`run_ctx`] for commands that read a payload off
/// stdin (`ctx traits hook`, P499) — `run_ctx`'s `.output()` gives the child
/// a null stdin, which is wrong for those.
pub fn run_ctx_with_stdin(args: &[&str], cwd: &Path, home: &Path, stdin: &[u8]) -> Output {
    let mut child = controlled_command(&ctx_bin(), args, cwd, home)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("cannot execute {}: {error}", ctx_bin().display()));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin)
        .unwrap_or_else(|error| panic!("cannot write stdin: {error}"));
    child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("cannot wait for {}: {error}", ctx_bin().display()))
}

/// Decode `output`'s stdout/stderr as UTF-8, panicking with the raw bytes on
/// failure — CLI output is a text contract, so lossless UTF-8 is a fair
/// assumption to enforce here rather than silently lossy-decoding a proof's
/// actual bytes.
/// Assert a completed process exited with `expected`, panicking with the
/// captured stdout/stderr so a failing test names what the binary actually
/// said. Restored at the P460 landing: the helper left `support` together
/// with the deleted insta demo (its then-only consumer), while P460's
/// branch-side `proof_merge_on_completion` was authored against it.
pub fn assert_exit_code(output: &Output, expected: i32) {
    let (stdout, stderr) = utf8(output);
    assert_eq!(
        output.status.code(),
        Some(expected),
        "expected exit {expected}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

pub fn utf8(output: &Output) -> (String, String) {
    let stdout = String::from_utf8(output.stdout.clone())
        .unwrap_or_else(|error| panic!("stdout was not valid UTF-8: {error}"));
    let stderr = String::from_utf8(output.stderr.clone())
        .unwrap_or_else(|error| panic!("stderr was not valid UTF-8: {error}"));
    (stdout, stderr)
}

/// Run `ctx` with `args` in `cwd`/`home`, panicking with `description` and
/// the full exit code/stdout/stderr report if it does not exit 0, and
/// otherwise returning its decoded stdout.
pub fn require_success(description: &str, args: &[&str], cwd: &Path, home: &Path) -> String {
    let output = run_ctx(args, cwd, home);
    let (stdout, stderr) = utf8(&output);
    assert!(
        output.status.success(),
        "{description} exited {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code()
    );
    stdout
}

/// Like [`require_success`], but layering `extra_env` via [`run_ctx_with_env`].
pub fn require_success_with_env(
    description: &str,
    args: &[&str],
    cwd: &Path,
    home: &Path,
    extra_env: &[(&str, &str)],
) -> String {
    let output = run_ctx_with_env(args, cwd, home, extra_env);
    let (stdout, stderr) = utf8(&output);
    assert!(
        output.status.success(),
        "{description} exited {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code()
    );
    stdout
}

/// Turn an already-created scratch project directory into a real (empty) Git
/// repository on branch `main` with a fixed identity, matching
/// `scripts/byte_compare.rs`'s `scratch_git_init`: repo-authored
/// project-tier resolution only consults the project tier inside a genuine
/// Git repository, so a bare directory with just `.ctx/traits/config.toml` does not
/// qualify.
pub fn git_init(dir: &Path) {
    git_init_on_branch(dir, "main");
}

/// Write a fixture trait's canonical file and generated manifest, keyed on
/// one activation keyword. Shared by the P499/P501 hook proof suites
/// (`proof_claude_code_hook`, `proof_codex_hook`) — the harnesses' proofs
/// need byte-identical fixture traits so a divergence in fixture-building
/// never masquerades as a harness-specific behavior difference.
pub fn write_hook_fixture_trait(repo: &Path, id: &str, name: &str, keyword: &str, summary: &str) {
    write_fixture_file(
        &repo.join(format!(".ctx/traits/authored/{id}/trait.toml")),
        &format!(
            "[package]\nid = {id:?}\nversion = \"0.1.0\"\nname = {name:?}\nstatus = \"draft\"\n"
        ),
    );
    write_fixture_file(
        &repo.join(format!(".ctx/traits/authored/{id}/generated/index.toml")),
        &hook_fixture_manifest(id, name, keyword, summary),
    );
}

fn hook_fixture_manifest(id: &str, name: &str, keyword: &str, summary: &str) -> String {
    format!(
        "id = \"{id}\"\n\
schema-version = \"0.2\"\n\
version = \"0.1.0\"\n\
name = {name:?}\n\
summary = {summary:?}\n\
\n\
[activation]\n\
\n\
[[activation.rule]]\n\
id = \"always\"\n\
reason = \"matches the fixture task\"\n\
task-keyword = \"{keyword}\"\n"
    )
}

fn write_fixture_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    std::fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// Clear a hook fixture trait's draft/unreviewed gates so the hook's
/// render-trust gate never refuses it.
pub fn ready_hook_fixture_trait(repo: &Path, home: &Path, id: &str) {
    require_success(
        "`ctx traits activate` clears the draft gate",
        &["traits", "activate", id],
        repo,
        home,
    );
    require_success(
        "`ctx traits trust approve` clears the unreviewed gate",
        &["traits", "trust", "approve", id],
        repo,
        home,
    );
}

/// The wall-clock-carrying JSON field names a persisted run-session ledger
/// may emit (`modules/core/src/procedure/runtime/state.rs`,
/// `modules/io/src/debug_trace.rs`): pinning exactly these, by name, keeps a
/// two-run-equivalence assertion honest — every other byte still compares
/// exactly, rather than falling back to a blanket diff that could mask an
/// unrelated regression. P461's standing FLAKE-ledger fix: the 2026-07-24
/// pin covered `recorded-at-epoch` only and the suite still flaked (a
/// second wall-clock field crossing a second boundary); this is the
/// generalized helper every two-run-equivalence proof should use going
/// forward.
const VOLATILE_LEDGER_FIELDS: &[&str] = &[
    "recorded-at-epoch",
    "elapsed-seconds",
    "elapsed-seconds-at-least",
    "duration-ms",
    "accepted-at",
];

/// Replace every occurrence of `"<field>": <number>` for each volatile
/// ledger field name (see this module's private `VOLATILE_LEDGER_FIELDS`)
/// with `"<field>": 0`, leaving every other byte untouched.
pub fn pin_volatile_ledger_fields(text: &str) -> String {
    let mut pinned = text.to_string();
    for field in VOLATILE_LEDGER_FIELDS {
        let marker = format!("\"{field}\": ");
        let mut rewritten = String::with_capacity(pinned.len());
        let mut rest = pinned.as_str();
        while let Some(index) = rest.find(&marker) {
            let value_start = index + marker.len();
            rewritten.push_str(&rest[..value_start]);
            rewritten.push('0');
            rest = rest[value_start..].trim_start_matches(|c: char| c.is_ascii_digit());
        }
        rewritten.push_str(rest);
        pinned = rewritten;
    }
    pinned
}

/// Runs `ctx <args>` under `expect` on a sized PTY, answering every
/// crossterm `ESC[6n` cursor-position query with a synthetic reply so the
/// inline pane's construction never stalls, then reports the child's own
/// exit code (recovered from `expect`'s `[wait]`, not `expect`'s own status)
/// alongside the full raw stdout stream. Extracted after this recipe was
/// copied inline into two proof files (`proof_run_startup_progress.rs`,
/// `proof_task_queue_refusal_teardown.rs`); a third proof
/// (`proof_task_queue_pane_handoff.rs`) and both prior call sites now share
/// this implementation instead.
pub fn run_pty_with_cursor_reply(
    binary: &Path,
    args: &str,
    cwd: &Path,
    home: &Path,
    marker: &str,
    termios_file: &str,
) -> (i32, String) {
    let output = Command::new("expect")
        .args([
            "-c",
            &format!(
                r#"
                set timeout 30
                set child_status {{}}
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; $env(CTX_STARTUP_BIN) {args}; status=\$?; stty -a > {termios_file}; printf '{marker}\n'; exit \$status"
                expect {{
                    -re {{\x1b\[6n}} {{ send -- "\033\[40;120R"; exp_continue }}
                    eof {{ set child_status [wait] }}
                }}
                puts "__CHILD_EXIT__[lindex $child_status 3]__"
            "#
            ),
        ])
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_CACHE_HOME", home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("TERM", "xterm-256color")
        .env("CTX_STARTUP_BIN", binary)
        .output()
        .unwrap();
    assert!(output.status.success(), "PTY driver failed: {output:?}");
    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let exit_tag_start = raw
        .find("__CHILD_EXIT__")
        .unwrap_or_else(|| panic!("expect never reported the child's exit code: {raw:?}"));
    let after_tag = &raw[exit_tag_start + "__CHILD_EXIT__".len()..];
    let exit_code: i32 = after_tag[..after_tag.find("__").unwrap()].parse().unwrap();
    (exit_code, raw)
}

/// `restore_terminal`'s `Show` (`\x1b[?25h`) is the one escape sequence an
/// inline pane's teardown always emits and nowhere else on this path — so
/// slicing the raw PTY stream at its *last* occurrence, before stripping
/// escapes, proves content arrived on a cooked terminal after the pane
/// actually handed the screen back, not merely somewhere in the byte
/// history before a later repaint could have painted over it.
pub fn text_after_terminal_restore(raw: &str) -> String {
    const CURSOR_SHOW: &str = "\u{1b}[?25h";
    let boundary = raw
        .rfind(CURSOR_SHOW)
        .unwrap_or_else(|| panic!("terminal restore (cursor show) escape never appeared: {raw:?}"))
        + CURSOR_SHOW.len();
    strip_escapes(&raw[boundary..])
}

pub fn git_init_on_branch(dir: &Path, branch: &str) {
    for args in [
        &["init", "--quiet"][..],
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")][..],
        &["config", "user.name", "ctx-traits-cli-test"][..],
        &[
            "config",
            "user.email",
            "ctx-traits-cli-test@example.invalid",
        ][..],
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|error| panic!("cannot run git {}: {error}", args.join(" ")));
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
