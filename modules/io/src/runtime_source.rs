//! `RuntimeConfig`'s generated TypeScript-authoring pathway (0178), mirroring
//! `crate::config_document`'s `config.ts` -> `generated/config.toml`
//! pathway one-for-one for `runtime.ts` -> `generated/runtime.toml`, at
//! both the repo tier ([`layout::RUNTIME_CONFIG_SOURCE`]) and the global
//! tier ([`layout::GLOBAL_RUNTIME_CONFIG_SOURCE`]). Shares
//! `crate::config_source`'s source-manifest machinery; adds nothing new to
//! it — only the tier-specific path selection lives here.

use camino::{Utf8Path, Utf8PathBuf};

fn refusal(message: impl Into<String>) -> crate::Error {
    crate::Error::Core(
        ctx_traits_core::manifest::Error::InvalidField {
            field_path: "runtime".to_string(),
            message: message.into(),
        }
        .into(),
    )
}

/// Resolve which path a tier's `RuntimeConfig` document should be read
/// from, honoring the `source_path` (`runtime.ts`) -> `generated_path`
/// (`generated/runtime.toml`) pathway over `hand_path` (`runtime.toml`),
/// and refusing when both a source and a hand file are present at the same
/// tier (one authority per fact). Returns `Ok(None)` when neither exists —
/// callers treat that as "no document at this tier", not an error.
pub fn resolve_runtime_document_path(
    source_path: &Utf8Path,
    hand_path: &Utf8Path,
    generated_path: &Utf8Path,
) -> crate::Result<Option<Utf8PathBuf>> {
    if source_path.exists() {
        if hand_path.exists() {
            return Err(refusal(format!(
                "both {source_path} and hand-authored {hand_path} are present — one authority per fact: delete one"
            )));
        }
        crate::config_source::guard_never_built(source_path, generated_path)?;
        if !generated_path.exists() {
            return Ok(None);
        }
        let text = crate::read::read_text(generated_path)?;
        let source_dir = source_path
            .parent()
            .unwrap_or_else(|| Utf8Path::new(""))
            .to_path_buf();
        crate::config_source::guard_generated_config(generated_path, &text, &source_dir)?;
        return Ok(Some(generated_path.to_path_buf()));
    }

    if hand_path.exists() {
        return Ok(Some(hand_path.to_path_buf()));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn write(path: &Utf8Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn scratch_dir() -> Utf8PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "runtime-source-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Utf8PathBuf::from_path_buf(dir).unwrap()
    }

    #[test]
    fn no_source_no_hand_resolves_to_none() {
        let root = scratch_dir();
        let resolved = resolve_runtime_document_path(
            &root.join("runtime.ts"),
            &root.join("runtime.toml"),
            &root.join("generated/runtime.toml"),
        )
        .unwrap();
        assert_eq!(resolved, None);
    }

    #[test]
    fn hand_only_resolves_to_hand_path() {
        let root = scratch_dir();
        let hand = root.join("runtime.toml");
        write(&hand, "");
        let resolved = resolve_runtime_document_path(
            &root.join("runtime.ts"),
            &hand,
            &root.join("generated/runtime.toml"),
        )
        .unwrap();
        assert_eq!(resolved, Some(hand));
    }

    #[test]
    fn source_and_hand_both_present_refuses() {
        let root = scratch_dir();
        write(&root.join("runtime.ts"), "");
        write(&root.join("runtime.toml"), "");
        let err = resolve_runtime_document_path(
            &root.join("runtime.ts"),
            &root.join("runtime.toml"),
            &root.join("generated/runtime.toml"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("one authority per fact"));
    }

    #[test]
    fn source_never_built_refuses() {
        let root = scratch_dir();
        write(&root.join("runtime.ts"), "");
        let err = resolve_runtime_document_path(
            &root.join("runtime.ts"),
            &root.join("runtime.toml"),
            &root.join("generated/runtime.toml"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("config build"));
    }

    #[test]
    fn source_built_and_fresh_resolves_to_generated() {
        let root = scratch_dir();
        let source = root.join("runtime.ts");
        write(&source, "export default {}\n");
        let digest = crate::config_source::hash_file(&source).unwrap();
        let header = crate::config_source::render_header(&[crate::config_source::SourceEntry {
            path: Utf8PathBuf::from("runtime.ts"),
            digest,
        }]);
        let generated = root.join("generated/runtime.toml");
        write(&generated, &header);
        let resolved =
            resolve_runtime_document_path(&source, &root.join("runtime.toml"), &generated).unwrap();
        assert_eq!(resolved, Some(generated));
    }

    #[test]
    fn source_stale_refuses() {
        let root = scratch_dir();
        let source = root.join("runtime.ts");
        write(&source, "export default {}\n");
        let stale_digest = ctx_traits_core::digest::Digest::from_bytes(b"stale");
        let header = crate::config_source::render_header(&[crate::config_source::SourceEntry {
            path: Utf8PathBuf::from("runtime.ts"),
            digest: stale_digest,
        }]);
        let generated = root.join("generated/runtime.toml");
        write(&generated, &header);
        let err = resolve_runtime_document_path(&source, &root.join("runtime.toml"), &generated)
            .unwrap_err();
        assert!(err.to_string().contains("changed since"));
    }
}
