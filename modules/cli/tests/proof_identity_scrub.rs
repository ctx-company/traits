//! P490 done-when: no tracked file in the repository carries the authoring
//! machine's identity — an owner-account slug or an absolute host home
//! path — and no `.rs` source line in `modules/{cli,io,core}/src` embeds an
//! absolute host home path either.
//!
//! `git ls-files` is the primary sweep (the shipped-repo surface a
//! `git clone` or source tarball actually exposes); the source-tree walk is
//! a defense-in-depth belt for `modules/*/src` specifically. Every
//! survivor is on an explicit, justified allowlist keyed by file + matched
//! substring — nothing is exempted merely by being inside a `#[cfg(test)]`
//! block.

use std::process::Command;

use support::{collect_rs_files, repo_root};

/// One justified survivor of the identity scan.
struct Allowed {
    /// Repository-relative source file path.
    file: &'static str,
    /// Substring the offending line must contain to match this entry.
    contains: &'static str,
    /// One-line justification for why this is not a leaked identity.
    reason: &'static str,
}

const ALLOWLIST: &[Allowed] = &[
    Allowed {
        file: "modules/io/src/confinement.rs",
        contains: "HOME=/Users/<user>", // identity-guard-literal
        reason: "illustrative macOS HOME shape in doc prose, no authoring identity",
    },
    Allowed {
        file: "modules/io/src/layout/mod.rs",
        contains: "/home/u/", // identity-guard-literal
        reason: "#[cfg(test)] generic Linux path-shape fixture, not an authoring identity",
    },
    Allowed {
        file: "modules/core/build.rs",
        contains: "/Users/", // identity-guard-literal
        reason: "P490 identity-scrub guard's own doc/detection prefixes, not a leak",
    },
    Allowed {
        file: "modules/core/build.rs",
        contains: "/home/", // identity-guard-literal
        reason: "P490 identity-scrub guard's own doc/detection prefixes, not a leak",
    },
    Allowed {
        file: "modules/cli/tests/proof_identity_scrub.rs",
        contains: "// identity-guard-literal",
        reason: "the guard's own needle table and allowlist must spell out the literal shapes \
                  it detects; each exempt line is marked deliberately so every unmarked \
                  identity line in this file still fails",
    },
];

/// Absolute-home path shapes a committed file must never carry: the
/// authoring account's literal slug, and the generic macOS/Linux home
/// prefixes (a foreign author's home directory would leak just as much
/// identity as this repo's own).
const IDENTITY_NEEDLES: &[&str] = &["rpunkfu", "/Users/", "/home/"]; // identity-guard-literal

fn tracked_files(root: &std::path::Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("cannot run git ls-files: {error}"));
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_allowed(relative: &str, line: &str) -> bool {
    ALLOWLIST
        .iter()
        .any(|entry| entry.file == relative && line.contains(entry.contains))
}

#[test]
fn no_tracked_file_carries_authoring_identity() {
    let root = repo_root();
    let files = tracked_files(&root);
    assert!(
        files.len() > 50,
        "expected git ls-files to report a substantial number of tracked files, found {} — \
         the sweep likely ran from the wrong root",
        files.len()
    );

    let mut violations = Vec::new();
    for relative in &files {
        let path = root.join(relative);
        let Ok(text) = std::fs::read(&path) else {
            continue;
        };
        let Ok(text) = String::from_utf8(text) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if !IDENTITY_NEEDLES.iter().any(|needle| line.contains(needle)) {
                continue;
            }
            if is_allowed(relative, line) {
                continue;
            }
            violations.push(format!("{relative}:{}: {line}", index + 1));
        }
    }

    assert!(
        violations.is_empty(),
        "found tracked lines carrying the authoring machine's identity with no allowlist \
         justification (see modules/cli/tests/proof_identity_scrub.rs's ALLOWLIST):\n{}",
        violations.join("\n")
    );
}

#[test]
fn no_source_embeds_absolute_home_path() {
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
            let has_home_prefix = line.contains("/Users/") || line.contains("/home/"); // identity-guard-literal
            if !has_home_prefix {
                continue;
            }
            if is_allowed(&relative, line) {
                continue;
            }
            violations.push(format!("{relative}:{}: {line}", index + 1));
        }
    }

    assert!(
        violations.is_empty(),
        "found an absolute host home path embedded in modules/*/src with no allowlist \
         justification (see modules/cli/tests/proof_identity_scrub.rs's ALLOWLIST):\n{}",
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
