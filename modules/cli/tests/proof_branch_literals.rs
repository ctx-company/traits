//! P488 done-when 3: an rg-style gate holding zero literal `"main"` (or
//! `b"main"`) branch-ref string tokens across `modules/{cli,io,core}/src`.
//! Every landing path now discovers the default branch (see
//! `ctx_traits_io::worktree::resolve_default_branch`) rather than assuming
//! `"main"`; a reintroduced literal branch assumption is a regression this
//! gate catches immediately, by file:line, rather than silently.
//!
//! Doc-comment prose (`//`/`///` lines) is skipped: the gate targets string
//! literals actually used as branch refs at runtime, never conceptual prose
//! about what "main" commonly means. Every surviving literal is on an
//! explicit, justified allowlist keyed by file + matched substring — nothing
//! is exempted merely by being inside a `#[cfg(test)]` block.

use support::{collect_rs_files, repo_root};

/// One justified literal `"main"`/`b"main"` token survivor.
struct Allowed {
    /// Repository-relative source file path.
    file: &'static str,
    /// Substring the offending line must contain to match this entry.
    contains: &'static str,
    /// One-line justification for why this is not a branch ref.
    reason: &'static str,
}

const ALLOWLIST: &[Allowed] = &[
    Allowed {
        file: "modules/io/src/publish.rs",
        contains: "contains_key(\"main\")",
        reason: "npm package.json runtime-entry field name, not a branch",
    },
    Allowed {
        file: "modules/io/src/worktree.rs",
        contains: "(\"main\".to_string(), DefaultBranchSource::Fallback)",
        reason: "resolve_default_branch's one legitimate literal fallback value (P488)",
    },
    Allowed {
        file: "modules/cli/src/app/merge.rs",
        contains: "Some(b\"main\")",
        reason: "#[cfg(test)] deep_hunk_id byte-fixture input, not a branch ref",
    },
];

/// `true` for a line that is pure `//`/`///`/`//!` prose — the gate targets
/// string literals used as branch refs at runtime, never conceptual
/// commentary about what "main" commonly means.
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

#[test]
fn no_unjustified_literal_main_branch_refs() {
    let root = repo_root();
    let mut files = Vec::new();
    for relative in ["modules/cli/src", "modules/io/src", "modules/core/src"] {
        collect_rs_files(&root.join(relative), &mut files);
    }
    assert!(
        files.len() > 50,
        "expected the source-tree walk to find a substantial number of .rs files, found {} — \
         the walk likely resolved the wrong root",
        files.len()
    );

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
            if !line.contains("\"main\"") && !line.contains("b\"main\"") {
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
        "found literal \"main\"/b\"main\" branch-ref tokens with no allowlist justification \
         (see modules/cli/tests/proof_branch_literals.rs's ALLOWLIST):\n{}",
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
