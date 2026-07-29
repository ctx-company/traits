//! Generated-config marker/manifest format (P457).
//!
//! An optional `.ctx/config.ts` (or global `config.ts`) is a TypeScript
//! authoring source that `ctx traits config build` compiles into the sibling
//! `config.toml`. When a `config.ts` is present, the sibling `config.toml`
//! MUST be a generated artifact: a leading comment block naming a
//! `# ctx:generated` marker plus a `(path, sha256)` manifest over the full
//! transitive local module graph `config.ts` imports. Every config load
//! re-hashes exactly the listed files and refuses on drift. A repo with no
//! `config.ts` never parses this header and never hashes anything — the
//! hand-authored, TOML-first default stays byte-identical to before this
//! module existed.

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::digest::Digest;

pub const MARKER_LINE: &str = "# ctx:generated — built from config.ts by `ctx traits config build`; edit config.ts, not this file";
const SOURCE_PREFIX: &str = "# ctx:source ";
/// The authoring source file name, sibling to the generated `config.toml` in
/// every layer (repo-local and global).
pub const SOURCE_FILE_NAME: &str = "config.ts";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntry {
    /// Relative to the config file's own directory, forward-slash separated.
    pub path: Utf8PathBuf,
    pub digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedHeader {
    pub entries: Vec<SourceEntry>,
}

/// Sibling `config.ts` path for a given `config.toml`-named path (same
/// directory, `config.ts` in place of `config.toml`).
pub fn sibling_source_path(config_toml_path: &Utf8Path) -> Utf8PathBuf {
    let dir = config_toml_path
        .parent()
        .unwrap_or_else(|| Utf8Path::new(""));
    dir.join(SOURCE_FILE_NAME)
}

/// True when `path`'s file name is exactly `config.toml` — the only names
/// this guard applies to (never `harness.toml`, never the legacy
/// `ctx.toml`/`ctx-harness.toml` names).
pub fn is_generated_config_candidate(path: &Utf8Path) -> bool {
    path.file_name() == Some("config.toml")
}

/// Render the leading comment-block header for a freshly built `config.toml`.
/// `entries` must already be sorted by path (callers building the manifest
/// from a filesystem walk should sort before calling).
pub fn render_header(entries: &[SourceEntry]) -> String {
    let mut out = String::new();
    out.push_str(MARKER_LINE);
    out.push('\n');
    for entry in entries {
        out.push_str(SOURCE_PREFIX);
        out.push_str(entry.path.as_str());
        out.push(' ');
        out.push_str(&entry.digest.to_string());
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Parse the leading comment block of `text`. Returns `None` when the first
/// line is not the exact `# ctx:generated` marker — i.e. an ordinary,
/// hand-authored `config.toml` never has a header to parse. Stops at the
/// first non-comment, non-blank line, so an authored comment further down
/// the file can never forge a manifest entry.
pub fn parse_header(text: &str) -> Option<GeneratedHeader> {
    let mut lines = text.lines();
    let first = lines.next()?;
    if first != MARKER_LINE {
        return None;
    }
    let mut entries = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some(rest) = line.strip_prefix(SOURCE_PREFIX) else {
            break;
        };
        let (path, digest) = rest.rsplit_once(' ')?;
        let digest = Digest::parse(digest).ok()?;
        entries.push(SourceEntry {
            path: Utf8PathBuf::from(path),
            digest,
        });
    }
    Some(GeneratedHeader { entries })
}

/// Hash raw file bytes (never the normalized/source-digest form — this is
/// change detection over exact authored bytes, not canonical content).
pub fn hash_file(path: &Utf8Path) -> crate::Result<Digest> {
    let bytes = std::fs::read(path).map_err(|source| crate::environment::Error::Filesystem {
        path: path.to_string(),
        source,
    })?;
    Ok(Digest::from_bytes(&bytes))
}

fn refusal(message: impl Into<String>) -> crate::Error {
    crate::Error::Core(
        ctx_traits_core::manifest::Error::InvalidField {
            field_path: "config".to_string(),
            message: message.into(),
        }
        .into(),
    )
}

/// Reject absolute manifest paths on both write and read — a manifest entry
/// is always relative to the config file's own directory, so no
/// machine-specific absolute path is ever embedded in the artifact or
/// followed while re-hashing.
fn resolve_relative(config_dir: &Utf8Path, entry_path: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    if entry_path.is_absolute() {
        return Err(refusal(format!(
            "generated config manifest entry {entry_path} must be a relative path"
        )));
    }
    Ok(config_dir.join(entry_path))
}

/// Guard a `config.toml`-named path whose sibling `config.ts` exists but the
/// `config.toml` itself does not: it was never built.
pub fn guard_never_built(config_toml_path: &Utf8Path) -> crate::Result<()> {
    let source_path = sibling_source_path(config_toml_path);
    if source_path.exists() && !config_toml_path.exists() {
        return Err(refusal(format!(
            "{source_path} has no built {config_toml_path} — run `ctx traits config build`"
        )));
    }
    Ok(())
}

/// Guard an already-read `config.toml`'s text against its sibling
/// `config.ts`, if one exists. Reuses the text the caller already read — no
/// second filesystem read of the TOML file itself.
pub fn guard_config_toml(config_toml_path: &Utf8Path, config_toml_text: &str) -> crate::Result<()> {
    let source_path = sibling_source_path(config_toml_path);
    if !source_path.exists() {
        // No sibling source: ordinary TOML, zero check, zero node — the
        // default, most common case, and covers the seeded-worktree case
        // (a marked config.toml with no config.ts loads normally).
        return Ok(());
    }
    let Some(header) = parse_header(config_toml_text) else {
        return Err(refusal(format!(
            "{config_toml_path} was not generated from {source_path} (missing # ctx:generated marker) — run `ctx traits config build` to regenerate it, or delete {source_path} to go TOML-first"
        )));
    };
    let config_dir = config_toml_path
        .parent()
        .unwrap_or_else(|| Utf8Path::new(""));
    for entry in &header.entries {
        let resolved = resolve_relative(config_dir, &entry.path)?;
        let actual = match hash_file(&resolved) {
            Ok(digest) => digest,
            Err(_) => {
                return Err(refusal(format!(
                    "{} changed since {config_toml_path} was built (missing) — run `ctx traits config build`",
                    entry.path
                )));
            }
        };
        if actual != entry.digest {
            return Err(refusal(format!(
                "{} changed since {config_toml_path} was built — run `ctx traits config build`",
                entry.path
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let entries = vec![
            SourceEntry {
                path: Utf8PathBuf::from("config.ts"),
                digest: Digest::from_bytes(b"one"),
            },
            SourceEntry {
                path: Utf8PathBuf::from("../shared/pools.ts"),
                digest: Digest::from_bytes(b"two"),
            },
        ];
        let rendered = render_header(&entries);
        let text = format!("{rendered}[dummy]\nkey = 1\n");
        let parsed = parse_header(&text).expect("marker present");
        assert_eq!(parsed.entries, entries);
    }

    #[test]
    fn no_marker_returns_none() {
        assert!(parse_header("[dummy]\nkey = 1\n").is_none());
    }

    #[test]
    fn user_comment_below_header_is_not_a_manifest_entry() {
        let text = format!(
            "{MARKER_LINE}\n# ctx:source config.ts sha256:{}\n\n# ctx:source evil.ts sha256:{}\n[dummy]\n",
            "0".repeat(64),
            "1".repeat(64)
        );
        let parsed = parse_header(&text).expect("marker present");
        assert_eq!(parsed.entries.len(), 1);
    }
}
