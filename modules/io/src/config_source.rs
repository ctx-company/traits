//! Generated `ConfigDocument` source-manifest format (0177; retires P457's
//! in-file marker contract).
//!
//! `.ctx/traits/config.ts` is an optional TypeScript authoring source that
//! `ctx traits config build` compiles into
//! `.ctx/traits/generated/config.toml`. LOCATION states authority now — a
//! file under `generated/` is machine-written by construction, never
//! hand-edited — so there is no in-file marker to parse or forge. This
//! module keeps only the source-manifest machinery: a leading `# ctx:source
//! <path> <sha256>` comment block over the full transitive local module
//! graph `config.ts` imports, re-hashed on every read so drift refuses
//! naming the changed file. `crate::config_document` owns the
//! source/generated/hand-authored path selection this module's callers key
//! their checks on.

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::digest::Digest;

const SOURCE_PREFIX: &str = "# ctx:source ";
/// The authoring source file name, sibling to the config document's own
/// root (not to the generated artifact — see `crate::config_document`).
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

/// Render the leading `# ctx:source` comment block for a freshly built
/// generated config document. `entries` must already be sorted by path
/// (callers building the manifest from a filesystem walk should sort before
/// calling). No marker line: the file's location under `generated/` is
/// itself the authority signal.
pub fn render_header(entries: &[SourceEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(SOURCE_PREFIX);
        out.push_str(entry.path.as_str());
        out.push(' ');
        out.push_str(&entry.digest.to_string());
        out.push('\n');
    }
    if !entries.is_empty() {
        out.push('\n');
    }
    out
}

/// Parse the leading `# ctx:source` comment block of `text`. Stops at the
/// first non-comment, non-blank line, so an authored comment further down
/// the file can never forge a manifest entry. Returns an empty manifest
/// (never `None`) when no source lines lead the file — callers key whether
/// to parse at all on the document's *location*, not on this return value.
pub fn parse_header(text: &str) -> GeneratedHeader {
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        let Some(rest) = line.strip_prefix(SOURCE_PREFIX) else {
            break;
        };
        let Some((path, digest)) = rest.rsplit_once(' ') else {
            break;
        };
        let Ok(digest) = Digest::parse(digest) else {
            break;
        };
        entries.push(SourceEntry {
            path: Utf8PathBuf::from(path),
            digest,
        });
    }
    GeneratedHeader { entries }
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

/// Guard a config document location where `source_path` (`config.ts`)
/// exists but `generated_path` (`generated/config.toml`) does not: it was
/// never built.
pub fn guard_never_built(source_path: &Utf8Path, generated_path: &Utf8Path) -> crate::Result<()> {
    if source_path.exists() && !generated_path.exists() {
        return Err(refusal(format!(
            "{source_path} has no built {generated_path} — run `ctx traits config build`"
        )));
    }
    Ok(())
}

/// Guard an already-read generated document's text against its source-entry
/// manifest, re-hashing each listed file relative to `source_dir` (the
/// authoring source's own directory, not the generated file's directory —
/// the two are no longer siblings under 0177). Reuses the text the caller
/// already read — no second filesystem read of the TOML file itself.
pub fn guard_generated_config(
    generated_path: &Utf8Path,
    generated_text: &str,
    source_dir: &Utf8Path,
) -> crate::Result<()> {
    let header = parse_header(generated_text);
    for entry in &header.entries {
        let resolved = resolve_relative(source_dir, &entry.path)?;
        let actual = match hash_file(&resolved) {
            Ok(digest) => digest,
            Err(_) => {
                return Err(refusal(format!(
                    "{} changed since {generated_path} was built (missing) — run `ctx traits config build`",
                    entry.path
                )));
            }
        };
        if actual != entry.digest {
            return Err(refusal(format!(
                "{} changed since {generated_path} was built — run `ctx traits config build`",
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
        let parsed = parse_header(&text);
        assert_eq!(parsed.entries, entries);
    }

    #[test]
    fn no_source_lines_is_empty_manifest() {
        assert!(parse_header("[dummy]\nkey = 1\n").entries.is_empty());
    }

    #[test]
    fn user_comment_below_header_is_not_a_manifest_entry() {
        let text = format!(
            "# ctx:source config.ts sha256:{}\n\n# ctx:source evil.ts sha256:{}\n[dummy]\n",
            "0".repeat(64),
            "1".repeat(64)
        );
        let parsed = parse_header(&text);
        assert_eq!(parsed.entries.len(), 1);
    }
}
