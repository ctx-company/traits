//! P490 done-when (narrowed): no `.rs` source line in `modules/{cli,io,core}/src`
//! embeds the authoring machine's identity — an owner-account slug or an
//! absolute host home path. Every survivor is on an explicit, justified
//! allowlist keyed by file + matched substring.
//!
//! This proof deliberately scans only shipped source. It once swept every
//! `git ls-files` entry, which made any task-board document quoting an
//! absolute path (legitimate working notes under `.internal/`) fail the
//! workspace tests regardless of what code changed; that sweep is gone.
//! The other shipped surface — bytes embedded in the release binary — is
//! guarded at build time by `modules/core/build.rs`, which fails the build
//! if a builtin package byte carries a home-path prefix.

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
        contains: "HOME=/Users/<user>",
        reason: "illustrative macOS HOME shape in doc prose, no authoring identity",
    },
    Allowed {
        file: "modules/io/src/layout/mod.rs",
        contains: "/home/u/",
        reason: "#[cfg(test)] generic Linux path-shape fixture, not an authoring identity",
    },
];

/// Identity shapes shipped source must never carry: the authoring account's
/// literal slug, and the generic macOS/Linux home prefixes (a foreign
/// author's home directory would leak just as much identity as this repo's
/// own).
const IDENTITY_NEEDLES: &[&str] = &["rpunkfu", "/Users/", "/home/"];

fn is_allowed(relative: &str, line: &str) -> bool {
    ALLOWLIST
        .iter()
        .any(|entry| entry.file == relative && line.contains(entry.contains))
}

#[test]
fn no_source_embeds_authoring_identity() {
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
            if !IDENTITY_NEEDLES.iter().any(|needle| line.contains(needle)) {
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
        "found the authoring machine's identity embedded in modules/*/src with no allowlist \
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
