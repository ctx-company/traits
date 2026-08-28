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

use std::hash::{Hash, Hasher};
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

/// Build an Expect ERE that permits terminal escape sequences between every
/// painted character, without accepting a different printable payload.
pub fn painted_pattern(literal: &str) -> String {
    const ESCAPE_RUN: &str = r"(\x1b\[[0-9;?]*[@-~])*";
    let mut pattern = String::new();
    for (index, character) in literal.chars().enumerate() {
        if index > 0 {
            pattern.push_str(ESCAPE_RUN);
        }
        match character {
            // Differential terminal renders may leave a blank cell untouched
            // while moving the cursor across it, so painted whitespace need
            // not be emitted as a byte in the PTY stream.
            character if character.is_whitespace() => {
                pattern.push_str(r"(?:[[:space:]]|\x1b\[[0-9;?]*[@-~])*")
            }
            '{' => pattern.push_str(r"\173"),
            '}' => pattern.push_str(r"\175"),
            '\\' | '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '|' => {
                pattern.push('\\');
                pattern.push(character);
            }
            _ => pattern.push(character),
        }
    }
    pattern
}

/// Check the printable form of a PTY stream using the same literal supplied
/// to [`painted_pattern`].
pub fn painted_text_present(raw: &str, literal: &str) -> bool {
    let text = strip_escapes(raw);
    text.contains(literal)
        || text.contains(
            &literal
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>(),
        )
}

pub fn ctx_bin() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_BIN_EXE_ctx")
            .expect("cargo test sets CARGO_BIN_EXE_ctx for ctx-traits-cli's integration tests"),
    )
}

/// Submit caller-provided fields through the persisted `internal call` boundary.
/// Identity fields come from the current frame template so every proof follows
/// the same stale-frame protection as a real caller.
pub fn call_session_frame(
    repo: &Path,
    home: &Path,
    ledger_path: &Path,
    agent: Option<&str>,
    fields: serde_json::Value,
) -> serde_json::Value {
    let ledger: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ledger_path).expect("fixture session ledger readable"),
    )
    .expect("fixture session ledger is JSON");
    let template = &ledger["next-frame"]["call-template"];
    let mut data = serde_json::json!({
        "session-id": template["session-id"],
        "run-id": template["run-id"],
        "state-digest": template["state-digest"],
        "expected-sequence-item-id": template["expected-sequence-item-id"],
        "expected-run-index": template["expected-run-index"],
        "expected-source-index": template["expected-source-index"],
    });
    let object = data
        .as_object_mut()
        .expect("fixture call payload is an object");
    if let Some(position_path) = template["expected-position-path"].as_array() {
        object.insert(
            "expected-position-path".to_string(),
            position_path.clone().into(),
        );
    }
    object.extend(
        fields
            .as_object()
            .expect("caller fields are an object")
            .clone(),
    );
    let data_path = ledger_path.with_extension("call.json");
    std::fs::write(&data_path, data.to_string()).expect("fixture call payload writable");
    let mut args = vec![
        "traits".to_string(),
        "internal".to_string(),
        "call".to_string(),
        "--session".to_string(),
        ledger_path.to_str().unwrap().to_string(),
    ];
    if let Some(agent) = agent {
        args.extend(["--agent".to_string(), agent.to_string()]);
    }
    args.extend([
        "--data".to_string(),
        data_path.to_str().unwrap().to_string(),
        "--json".to_string(),
    ]);
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = run_ctx(&arg_refs, repo, home);
    assert!(
        output.status.success(),
        "persisted call must succeed: {:?}",
        utf8(&output)
    );
    let (stdout, _) = utf8(&output);
    serde_json::from_str(&stdout).expect("persisted call returns JSON")
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
/// install — the one local dependency `ctx traits internal config build` needs on
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

/// A private `node_modules` for a fixture project: every entry of the
/// repository's own installed `node_modules` symlinked in individually
/// (never the whole directory — [`symlink_node_modules`] does that and
/// shares one real directory across every caller, which would make a
/// caller's own `@fixture/dep` package land inside the *repository's* real
/// `node_modules`), so a caller can add its own real package directories
/// alongside them. Shared by `proof_trait_composition` and `proof_fork`
/// (previously duplicated verbatim in both).
pub fn private_node_modules(proj: &Path) -> PathBuf {
    let node_modules = proj.join("node_modules");
    std::fs::create_dir_all(&node_modules).unwrap();
    for entry in std::fs::read_dir(repo_root().join("node_modules")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        #[cfg(unix)]
        std::os::unix::fs::symlink(entry.path(), node_modules.join(&name))
            .unwrap_or_else(|error| panic!("cannot symlink {name:?}: {error}"));
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(entry.path(), node_modules.join(&name))
            .unwrap_or_else(|error| panic!("cannot symlink {name:?}: {error}"));
    }
    node_modules
}

const FIXTURE_DEP_PACKAGE_JSON: &str =
    "{\n  \"name\": \"@fixture/dep\",\n  \"version\": \"1.0.0\"\n}\n";
const FIXTURE_DEP_SHARED_MJS: &str =
    "export const summaryText = \"Describe what this trait should accomplish.\";\n";

/// A minimal, independently synthesizable CDK source for `@fixture/dep`
/// itself — the dependency is a real trait package, built through the
/// ordinary `ctx traits init`/`build` lifecycle so its own `trait.lock` self
/// entry is written by production code (`ctx_traits_io::dependency::sync`'s
/// "self" load via `record_lock_evidence`), not hand-typed. `shared.mjs` is
/// the plain importable helper `@fixture/dep`'s *consumers* pull in — it is
/// not part of this trait's own canonical/port and is added to the package
/// root separately after this build.
fn fixture_dep_trait_source() -> &'static str {
    "import { agent, input, port, procedure, sequence, slot, trait } from \"@ctx-traits/cdk\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({ id: \"summary\", value: summary });\n\
const worker = agent(\"worker\", { description: \"Completes the starter task.\" });\n\
\n\
export const draft = trait({\n\
  id: \"fixture-dep\",\n\
  name: \"fixture-dep\",\n\
  description: \"A fixture dependency trait package.\",\n\
  port: output,\n\
  procedure: procedure({\n\
    description: \"A fixture dependency trait package.\",\n\
    sequence: sequence.prompt({\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: input.prompt`Describe the task for this trait.`,\n\
      output: summary,\n\
    }),\n\
  }),\n\
});\n"
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let dest_path = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&dest_path).unwrap();
            copy_dir_recursive(&entry.path(), &dest_path);
        } else {
            std::fs::copy(entry.path(), &dest_path).unwrap();
        }
    }
}

/// Builds `@fixture/dep` as its own real trait project in a scratch
/// directory (via `ctx traits init` + `build`, so its `trait.lock` self
/// entry is production-written), then copies the resulting package —
/// `trait.toml`, `trait.lock`, `generated/`, `source/` — into `dep_root`
/// alongside a `package.json` and the plain `shared.mjs` helper consumers
/// import. Shared by `proof_trait_composition` and `proof_fork` (previously
/// duplicated verbatim in both); `label` names the scratch root the
/// dependency build runs in.
pub fn write_locked_dependency_fixture(dep_root: &Path, label: &str) {
    let dep_scratch = ScratchRoot::new(label);
    let dep_home = dep_scratch.home();
    let dep_proj = dep_home.join("dep-repo");
    std::fs::create_dir_all(&dep_proj).unwrap();
    git_init(&dep_proj);
    private_node_modules(&dep_proj);

    require_success(
        "`ctx traits init fixture-dep` for the dependency fixture",
        &["traits", "init", "fixture-dep"],
        &dep_proj,
        &dep_home,
    );
    let dep_source_path = dep_proj.join(".ctx/traits/authored/fixture-dep/source/index.ts");
    std::fs::write(&dep_source_path, fixture_dep_trait_source()).unwrap();
    require_success(
        "building the dependency fixture's own trait package",
        &[
            "traits",
            "build",
            ".ctx/traits/authored/fixture-dep/source/index.ts",
        ],
        &dep_proj,
        &dep_home,
    );

    let dep_package_root = dep_proj.join(".ctx/traits/authored/fixture-dep");
    std::fs::create_dir_all(dep_root).unwrap();
    copy_dir_recursive(&dep_package_root, dep_root);
    std::fs::write(dep_root.join("package.json"), FIXTURE_DEP_PACKAGE_JSON).unwrap();
    std::fs::write(dep_root.join("shared.mjs"), FIXTURE_DEP_SHARED_MJS).unwrap();
    assert!(
        dep_root.join("trait.lock").exists(),
        "copying the dependency fixture's build output did not carry a trait.lock"
    );
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
    // Every scratch HOME needs its own complete center endpoint tuple. The
    // production socket is UID/version scoped, so HOME isolation alone would
    // otherwise allow one proof to query another proof's runs root.
    let center_root = home.join("ctx/traits/runs");
    // Darwin limits Unix-domain socket paths to 104 bytes. Scratch homes use
    // descriptive temp names, so put the endpoint under /tmp while retaining a
    // deterministic per-home identity for commands that share this harness.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    home.hash(&mut hasher);
    let center_socket = std::env::temp_dir().join(format!("ctx-{:016x}.sock", hasher.finish()));
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
        .env("CTX_CENTER_SOCKET", center_socket)
        .env("CTX_CENTER_SPAWN_LOCK", home.join("l"))
        .env("CTX_CENTER_RUNS_ROOT", &center_root)
        .env("CTX_CENTER_INDEX", center_root.join("index.sqlite3"))
        // 0262: the center recovers repository-local, no-driver-event
        // ledgers by seeding its warming scan from the driver-liveness
        // index. That index defaults to one fixed per-uid path shared by
        // every process on the machine; without scoping it here too, this
        // scratch center would seed from (and wade through) every other
        // proof's accumulated entries at that shared path.
        .env("CTX_CENTER_LIVENESS_ROOT", home.join("ctx/traits/liveness"))
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
/// stdin (`ctx traits internal hook`, P499) — `run_ctx`'s `.output()` gives the child
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
        "`ctx traits state --active` clears the draft gate",
        &["traits", "state", "--active", id],
        repo,
        home,
    );
    require_success(
        "`ctx traits trust --approved` clears the unreviewed gate",
        &["traits", "trust", "--approved", id],
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

const READY_BUDGET_MS: i64 = 30_000;
const CHILD_EXIT_GRACE_SECS: i64 = 60;
const PTY_TIMEOUT: &str = "__PTY_TIMEOUT__";
const PTY_EOF_BEFORE: &str = "__PTY_EOF_BEFORE__";
const PTY_CHILD_HANG: &str = "__PTY_CHILD_HANG__";
const CHILD_EXIT: &str = "__CHILD_EXIT__";

fn pty_prelude() -> String {
    format!(
        r#"
                set child_status {{}}
                set deadline [expr {{[clock milliseconds] + {READY_BUDGET_MS}}}]
                proc remaining {{what}} {{
                    set left [expr {{($::deadline - [clock milliseconds] + 999) / 1000}}]
                    if {{$left <= 0}} {{ puts stderr "__PTY_TIMEOUT__${{what}}__"; exit 2 }}
                    return $left
                }}
        "#
    )
}

fn marker_wait(pattern: &str, action: &str) -> String {
    format!(
        r#"
                set marker {{{pattern}}}
                set timeout [remaining $marker]
                expect {{
                    -re {{\x1b\[6n}} {{ send -- "\033\[40;120R"; exp_continue -continue_timer }}
                    -re $marker {{ {action} }}
                    timeout {{ puts stderr "__PTY_TIMEOUT__${{marker}}__"; exit 2 }}
                    eof {{ puts stderr "__PTY_EOF_BEFORE__${{marker}}__"; exit 2 }}
                }}
        "#
    )
}

fn child_lifetime_wait(resignal: Option<(&str, &str, usize)>) -> String {
    let resignal_arm = resignal.map_or_else(String::new, |(pattern, signal, repeat)| {
        format!(
            r#"
                    -re {{{pattern}}} {{ for {{set i 0}} {{$i < {repeat}}} {{incr i}} {{ exec kill -{signal} [exp_pid] }}; exp_continue -continue_timer }}
            "#
        )
    });
    format!(
        r#"
                set timeout {CHILD_EXIT_GRACE_SECS}
                expect {{
                    -re {{\x1b\[6n}} {{ send -- "\033\[40;120R"; exp_continue -continue_timer }}
                    {resignal_arm}
                    timeout {{ puts stderr "__PTY_CHILD_HANG__"; exit 2 }}
                    eof {{ set child_status [wait] }}
                }}
                puts stderr "__CHILD_EXIT__[lindex $child_status 3]__"
        "#
    )
}

fn pty_record(stderr: &str, prefix: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        if line == prefix {
            Some(String::new())
        } else {
            line.strip_prefix(prefix)
                .and_then(|payload| payload.strip_suffix("__"))
                .map(str::to_owned)
        }
    })
}

fn run_expect(script: &str, binary: &Path, cwd: &Path, home: &Path) -> (i32, String) {
    let output = controlled_command(Path::new("expect"), &["-c", script], cwd, home)
        // `expect` propagates its environment; TUI capability rejects any NO_COLOR value.
        .env_remove("NO_COLOR")
        .env("TERM", "xterm-256color")
        .env("CTX_STARTUP_BIN", binary)
        .output()
        .unwrap_or_else(|error| panic!("cannot execute expect: {error}"));
    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("PTY stdout was not valid UTF-8: {error}"));
    let stderr = String::from_utf8(output.stderr)
        .unwrap_or_else(|error| panic!("PTY stderr was not valid UTF-8: {error}"));
    let diagnostic = format!(
        "stdout:\n{}\nstderr:\n{}",
        strip_escapes(&stdout),
        strip_escapes(&stderr)
    );

    if let Some(marker) = pty_record(&stderr, PTY_TIMEOUT) {
        panic!(
            "PTY readiness marker {marker:?} did not appear within {READY_BUDGET_MS}ms\n{diagnostic}"
        );
    }
    if let Some(marker) = pty_record(&stderr, PTY_EOF_BEFORE) {
        panic!("PTY child exited before readiness marker {marker:?}\n{diagnostic}");
    }
    if pty_record(&stderr, PTY_CHILD_HANG).is_some() {
        panic!(
            "PTY child did not exit within {CHILD_EXIT_GRACE_SECS}s after the last key/signal\n{diagnostic}"
        );
    }
    assert!(
        output.status.success(),
        "PTY driver failed with {:?}\n{diagnostic}",
        output.status.code()
    );
    let exit_code = pty_record(&stderr, CHILD_EXIT)
        .unwrap_or_else(|| panic!("expect never reported the child's exit code\n{diagnostic}"))
        .parse()
        .unwrap_or_else(|error| {
            panic!("invalid child exit code in Expect stderr: {error}\n{diagnostic}")
        });
    (exit_code, stdout)
}

/// Runs `ctx <args>` under `expect` on a sized PTY, answering every
/// crossterm `ESC[6n` cursor-position query with a synthetic reply so the
/// surface that still issues it never stalls, then reports the child's own
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
    let script = format!(
        r#"
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; $env(CTX_STARTUP_BIN) {args}; status=\$?; stty -a > {termios_file}; printf '{marker}\n'; exit \$status"
                {}
                {}
        "#,
        pty_prelude(),
        child_lifetime_wait(None)
    );
    run_expect(&script, binary, cwd, home)
}

/// Runs a PTY child directly, waits for a painted marker, then signals that
/// child (rather than the shell wrapper used by `run_pty_with_cursor_reply`).
pub fn run_pty_signal_after_marker(
    binary: &Path,
    args: &str,
    cwd: &Path,
    home: &Path,
    ready_pattern: &str,
    signal: &str,
    repeat: usize,
) -> (i32, String) {
    let signal_child =
        format!("for {{set i 0}} {{$i < {repeat}}} {{incr i}} {{ exec kill -{signal} [exp_pid] }}");
    let script = format!(
        r#"
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; exec $env(CTX_STARTUP_BIN) {args}"
                {}
                {}
                {}
        "#,
        pty_prelude(),
        marker_wait(ready_pattern, &signal_child),
        child_lifetime_wait(Some((ready_pattern, signal, repeat)))
    );
    run_expect(&script, binary, cwd, home)
}

/// Runs a PTY child directly and sends each key only after its ready marker.
pub fn run_pty_keys_after_markers(
    binary: &Path,
    args: &str,
    cwd: &Path,
    home: &Path,
    steps: &[(&str, &str)],
) -> (i32, String) {
    let mut waits = String::new();
    for (pattern, key) in steps {
        waits.push_str(&marker_wait(pattern, &format!("send -- {{{key}}}")));
    }
    let script = format!(
        r#"
                spawn -noecho /bin/sh -c "stty cols 120 rows 40; exec $env(CTX_STARTUP_BIN) {args}"
                {}
                {waits}
                {}
            "#,
        pty_prelude(),
        child_lifetime_wait(None)
    );
    run_expect(&script, binary, cwd, home)
}

/// Slicing after the restore paired with the final alternate-screen entry
/// proves content arrived after the pane handed the screen back. A panic can
/// trigger an idempotent unwind restore later, so the final leave alone is not
/// a reliable boundary. Inline panes use their final cursor-show directly.
pub fn raw_after_terminal_restore(raw: &str) -> &str {
    const CURSOR_SHOW: &str = "\u{1b}[?25h";
    const ENTER_ALT: &str = "\u{1b}[?1049h";
    const LEAVE_ALT: &str = "\u{1b}[?1049l";
    let boundary = match raw.rfind(ENTER_ALT) {
        Some(enter) => {
            let after_enter = enter + ENTER_ALT.len();
            let leave = raw[after_enter..]
                .find(LEAVE_ALT)
                .unwrap_or_else(|| panic!("alternate screen was never left: {raw:?}"));
            let after_leave = after_enter + leave + LEAVE_ALT.len();
            let show = raw[after_leave..].find(CURSOR_SHOW).unwrap_or_else(|| {
                panic!("cursor was never shown after alternate-screen leave: {raw:?}")
            });
            after_leave + show
        }
        None => raw.rfind(CURSOR_SHOW).unwrap_or_else(|| {
            panic!("terminal restore (cursor show) escape never appeared: {raw:?}")
        }),
    } + CURSOR_SHOW.len();
    &raw[boundary..]
}

pub fn text_after_terminal_restore(raw: &str) -> String {
    strip_escapes(raw_after_terminal_restore(raw))
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
