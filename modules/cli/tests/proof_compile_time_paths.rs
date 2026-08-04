//! Task 0099 done-when: no `env!("CARGO_MANIFEST_DIR")` (or `option_env!`
//! variant) survives anywhere under `modules/**`. That pattern bakes a
//! worktree-specific absolute path into a compiled artifact; under a shared
//! build-slot `CARGO_TARGET_DIR`, a cached test binary compiled by one
//! worktree can then run against another worktree — or a pruned one — and
//! resolve paths that no longer exist (run-4de40dda, 0082). Every legitimate
//! test-root resolution instead reads `CARGO_MANIFEST_DIR` at run time via
//! `std::env::var`, which is a real process env var `cargo test` sets fresh
//! for each invocation.
//!
//! Doc-comment prose (`//`/`///`/`//!` lines) is skipped: the gate targets
//! the macro token actually compiled into the binary, never prose
//! describing the rule (including this file's own).

use support::{collect_rs_files, repo_root};

/// One justified survivor of the compile-time-bake pattern.
struct Allowed {
    /// Repository-relative source file path.
    file: &'static str,
    /// Substring the offending line must contain to match this entry.
    contains: &'static str,
    /// One-line justification for why this is not a test-root bake.
    reason: &'static str,
}

// `modules/core/build.rs` generates Rust source as a string with its quotes
// escaped (`env!(\"CARGO_MANIFEST_DIR\")`, backslash-quote, not a plain
// double-quote) — that is a different byte sequence from this proof's
// needle, which looks for the token as it appears directly in compiled
// source, so build.rs's occurrences never match and need no allowlist
// entry. (Those occurrences are a build-script compile-time embed of
// committed builtin trait/template bytes into the binary itself — not a
// test resolving its own root, so they carry none of this class's
// stale-worktree failure mode regardless.) Pre-flight sweep for task 0099
// confirmed the four fixed sites were the complete unescaped set; expected
// empty until a genuinely new bake surfaces.
const ALLOWLIST: &[Allowed] = &[];

/// The needle this proof scans for, assembled at run time so this file's own
/// source never contains the literal pattern it polices (which would
/// otherwise flag itself).
fn needle() -> String {
    ["env", "!", "(", "\"", "CARGO_MANIFEST_DIR", "\"", ")"].concat()
}

fn option_needle() -> String {
    [
        "option_",
        "env",
        "!",
        "(",
        "\"",
        "CARGO_MANIFEST_DIR",
        "\"",
        ")",
    ]
    .concat()
}

/// `true` for a line that is pure `//`/`///`/`//!` prose.
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

#[test]
fn needle_reassembles_to_the_expected_token() {
    assert_eq!(needle(), "env!(\"CARGO_MANIFEST_DIR\")");
    assert_eq!(option_needle(), "option_env!(\"CARGO_MANIFEST_DIR\")");
}

#[test]
fn no_unjustified_compile_time_manifest_dir_bakes() {
    let root = repo_root();
    let mut files = Vec::new();
    collect_rs_files(&root.join("modules"), &mut files);
    assert!(
        files.len() > 100,
        "expected the source-tree walk to find a substantial number of .rs files, found {} — \
         the walk likely resolved the wrong root",
        files.len()
    );

    let env_needle = needle();
    let option_env_needle = option_needle();
    let mut violations = Vec::new();
    for path in &files {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for (index, line) in text.lines().enumerate() {
            if is_comment_line(line) {
                continue;
            }
            if !line.contains(&env_needle) && !line.contains(&option_env_needle) {
                continue;
            }
            let allowed = ALLOWLIST
                .iter()
                .any(|entry| entry.file == relative && line.contains(entry.contains));
            if !allowed {
                violations.push(format!("{relative}:{}: {line}", index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "found compile-time CARGO_MANIFEST_DIR bakes with no allowlist justification (see \
         modules/cli/tests/proof_compile_time_paths.rs's ALLOWLIST):\n{}",
        violations.join("\n")
    );
}

#[test]
fn allowlist_entries_still_match_something() {
    let root = repo_root();
    for entry in ALLOWLIST {
        let path = root.join(entry.file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        assert!(
            text.lines().any(|line| line.contains(entry.contains)),
            "allowlist entry for {} ({:?}, {}) no longer matches any line — remove the stale \
             entry",
            entry.file,
            entry.contains,
            entry.reason
        );
    }
}
