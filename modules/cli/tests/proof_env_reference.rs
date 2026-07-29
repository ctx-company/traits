//! P492: `ctx_traits_io::env_reference::env_reference()` is the single
//! shipped inventory of every environment variable product code reads. This
//! walks `modules/*/src/**/*.rs` for quoted `CTX_`-prefixed string literals
//! (the full documented name grammar: uppercase ASCII, digits, underscore)
//! and asserts each one appears in that table — the same completeness
//! guarantee `just testhook-absence-check` relies on for the four test-hook
//! names, and the only thing standing between a newly-added `CTX_*` variable
//! and it silently going undocumented.

use std::fs;
use std::path::Path;

use support::repo_root;

/// One `"CTX_...` occurrence found while scanning: either a clean name (the
/// whole span from `CTX_` to a closing quote is valid grammar) or an
/// anomaly — a `CTX_`-prefixed quoted literal whose content is NOT a
/// `format!`-built prompt-delimiter marker (identified by an embedded `{`,
/// per `frame_prompt.rs`'s `format!("CTX_TRAITS_{kind}_{suffix}")`) and yet
/// does not fully match the grammar either. A completeness guard that can
/// drop an input without saying so cannot establish completeness, so an
/// anomaly is reported rather than silently discarded.
enum CtxLiteral {
    Name(String),
    Anomaly(String),
}

/// Scan `text` for quoted `CTX_`-prefixed literals, classifying each as a
/// documentable [`CtxLiteral::Name`] or a reported [`CtxLiteral::Anomaly`].
/// Requires the full quoted span (from `CTX_` to the next `"`) to be
/// considered a name — a partial match (e.g. only the character grammar
/// changed) is never silently trusted.
fn find_ctx_literals(text: &str) -> Vec<CtxLiteral> {
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(offset) = text[index..].find("\"CTX_") {
        let start = index + offset + 1; // skip the opening quote
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_uppercase()
                || bytes[end].is_ascii_digit()
                || bytes[end] == b'_')
        {
            end += 1;
        }
        // Bound the closing-quote search to the current line: a `"CTX_`
        // opener with no closing quote before the next newline is not a
        // single-line string literal this scan can meaningfully classify,
        // so it is skipped rather than matched against an unrelated quote
        // many lines later.
        let rest_of_line = match text[end..].find('\n') {
            Some(newline) => &text[end..end + newline],
            None => &text[end..],
        };
        let Some(close) = rest_of_line.find('"') else {
            index = start;
            continue;
        };
        let content = &text[start..end + close];
        if bytes.get(end) == Some(&b'"') {
            found.push(CtxLiteral::Name(content.to_string()));
        } else if !content.contains('{') {
            // Not a clean name, and not the known `format!("CTX_TRAITS_
            // {kind}_{suffix}")` prompt-delimiter shape (which always
            // contains `{`) — an unrecognized shape the scanner's grammar
            // does not yet account for.
            found.push(CtxLiteral::Anomaly(content.to_string()));
        }
        index = start;
    }
    found
}

fn walk_rust_sources(
    dir: &Path,
    names: &mut Vec<(String, String)>,
    anomalies: &mut Vec<(String, String)>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rust_sources(&path, names, anomalies);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            for literal in find_ctx_literals(&text) {
                match literal {
                    CtxLiteral::Name(name) => names.push((name, path.display().to_string())),
                    CtxLiteral::Anomaly(content) => {
                        anomalies.push((content, path.display().to_string()))
                    }
                }
            }
        }
    }
}

#[test]
fn every_ctx_env_literal_in_product_src_is_documented() {
    let modules_root = repo_root().join("modules");
    let mut literals = Vec::new();
    let mut anomalies = Vec::new();
    for entry in fs::read_dir(&modules_root).expect("modules/ exists") {
        let entry = entry.expect("readable modules/ entry");
        let src = entry.path().join("src");
        if src.is_dir() {
            walk_rust_sources(&src, &mut literals, &mut anomalies);
        }
    }

    anomalies.sort();
    anomalies.dedup();
    assert!(
        anomalies.is_empty(),
        "CTX_-prefixed quoted literal(s) the scanner's grammar could not classify as either a \
         documentable name or a known format!()-built prompt delimiter — widen \
         find_ctx_literals's grammar or the frame_prompt.rs delimiter check, do not ignore:\n{}",
        anomalies
            .iter()
            .map(|(content, file)| format!("\"{content}\" (in {file})"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let documented: std::collections::BTreeSet<&'static str> =
        ctx_traits_io::env_reference::env_reference()
            .into_iter()
            .map(|doc| doc.name)
            .collect();

    let mut missing = Vec::new();
    for (name, file) in &literals {
        if !documented.contains(name.as_str()) {
            missing.push(format!("{name} (in {file})"));
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "env var literal(s) missing from ctx_traits_io::env_reference::env_reference():\n{}",
        missing.join("\n")
    );
}

/// Pins the name grammar against the exact digit-bearing shape the repo
/// mints today (`CTX_FIXTURE_P445_TOKENS`, `CTX_FIXTURE_P427_ARGV_LOG`,
/// `CTX_FIXTURE_P313_LOG_DIR`) so the scanner's accepted-character set and
/// its doc-comment-claimed grammar cannot silently drift apart again — this
/// is what `every_ctx_env_literal_in_product_src_is_documented` failed to
/// catch before digits were added to `find_ctx_literals`'s character walk.
#[test]
fn find_ctx_literals_captures_digit_bearing_names() {
    let source = r#"
        fn read() {
            let _ = std::env::var("CTX_FIXTURE_P445_TOKENS");
            let _ = std::env::var("CTX_PLAIN_NAME");
        }
    "#;
    let names: Vec<String> = find_ctx_literals(source)
        .into_iter()
        .filter_map(|literal| match literal {
            CtxLiteral::Name(name) => Some(name),
            CtxLiteral::Anomaly(_) => None,
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "CTX_FIXTURE_P445_TOKENS".to_string(),
            "CTX_PLAIN_NAME".to_string(),
        ],
        "digit-bearing CTX_ literal must be captured as a whole name, not truncated at the first digit"
    );
}

/// The end-to-end failure shape the blocker described: an undocumented
/// digit-bearing literal must make the completeness assertion fail and name
/// the missing variable, not pass silently.
#[test]
fn undocumented_digit_bearing_literal_is_reported_missing() {
    let documented: std::collections::BTreeSet<&'static str> =
        ctx_traits_io::env_reference::env_reference()
            .into_iter()
            .map(|doc| doc.name)
            .collect();
    let source = r#"std::env::var("CTX_FIXTURE_P445_TOKENS")"#;
    let names: Vec<String> = find_ctx_literals(source)
        .into_iter()
        .filter_map(|literal| match literal {
            CtxLiteral::Name(name) => Some(name),
            CtxLiteral::Anomaly(_) => None,
        })
        .collect();
    assert_eq!(names, vec!["CTX_FIXTURE_P445_TOKENS".to_string()]);
    assert!(
        !documented.contains(names[0].as_str()),
        "CTX_FIXTURE_P445_TOKENS is expected to be undocumented in this crate's reference — \
         if it now appears, pick a different undocumented fixture-only name for this test"
    );
}
