//! `runtime.example.ts`/`.toml` acceptance mechanics (0178, deliverable 2).
//!
//! A repo may commit `.ctx/traits/runtime.example.ts` (or the pre-existing
//! `.ctx/traits/runtime.example.toml`) as the seed for the machine-local
//! `RUNTIME_CONFIG_SOURCE`/`RUNTIME_CONFIG` document. Run-dispatching
//! commands must refuse until the operator has explicitly accepted that
//! example's exact bytes — `ctx traits internal config accept` — which materializes
//! the machine-local copy and records a digest stamp in the existing
//! `crate::trust` store. The stamp covers the EXAMPLE's bytes, never the
//! machine copy: editing the accepted copy never re-prompts, but a changed
//! example (new digest) refuses again until re-accepted.

use camino::{Utf8Path, Utf8PathBuf};

use crate::trust::{self, TrustState};

/// Reserved trust-store trait-id namespace for runtime-example acceptance
/// stamps, keyed per-repo so a config-home-shared store still separates
/// checkouts. Distinct from real trait ids (which never contain `:`
/// followed by a `repo-key`-shaped suffix in this position), and excluded
/// from `trust --list`'s trait reporting by that same prefix.
const ACCEPTANCE_NAMESPACE: &str = "runtime-example";

fn acceptance_trait_id(repo_key: &str) -> String {
    format!("{ACCEPTANCE_NAMESPACE}:{repo_key}")
}

/// Result of checking whether the current example has been accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acceptance {
    /// No committed example exists at this tier: nothing to accept.
    NoExample,
    /// The example's current digest matches the latest verified stamp.
    Accepted,
    /// The example exists but its current digest has no verified stamp
    /// (never accepted, or accepted then changed).
    NeedsAcceptance {
        digest: String,
        prior_digest: Option<String>,
    },
}

/// Locate the single committed example under `root` (repo root or the
/// config-home tier root), refusing when both the `.ts` and `.toml`
/// examples are present — one authority per fact, per
/// [`crate::runtime_source`]'s sibling refusal for the machine document
/// itself.
pub fn find_example(
    example_ts: &Utf8Path,
    example_toml: &Utf8Path,
) -> crate::Result<Option<Utf8PathBuf>> {
    let ts_exists = example_ts.exists();
    let toml_exists = example_toml.exists();
    if ts_exists && toml_exists {
        return Err(crate::Error::Core(
            ctx_traits_core::manifest::Error::InvalidField {
                field_path: "runtime".to_string(),
                message: format!(
                    "both {example_ts} and {example_toml} are present — one authority per fact: delete one"
                ),
            }
            .into(),
        ));
    }
    if ts_exists {
        return Ok(Some(example_ts.to_path_buf()));
    }
    if toml_exists {
        return Ok(Some(example_toml.to_path_buf()));
    }
    Ok(None)
}

/// Check whether `example_path`'s current bytes are covered by a verified
/// acceptance stamp for `repo_key`.
pub fn check_acceptance(example_path: &Utf8Path, repo_key: &str) -> crate::Result<Acceptance> {
    let digest = crate::config_source::hash_file(example_path)?.to_string();
    let store = trust::read_store()?;
    let trait_id = acceptance_trait_id(repo_key);
    let prior = store.record_for_trait(&trait_id);
    let prior_digest = prior.map(|record| record.digest.clone());
    match store.exact_record(&trait_id, &digest) {
        Some(record) if record.state == TrustState::Verified => Ok(Acceptance::Accepted),
        _ => Ok(Acceptance::NeedsAcceptance {
            digest,
            prior_digest,
        }),
    }
}

/// Combined lookup + check for a single tier: `Ok(None)` means no example is
/// committed at this tier (nothing to gate); `Ok(Some(Acceptance))` reports
/// the acceptance state of whichever example was found.
pub fn guard_accepted(
    example_ts: &Utf8Path,
    example_toml: &Utf8Path,
    repo_key: &str,
) -> crate::Result<Option<Acceptance>> {
    let Some(example_path) = find_example(example_ts, example_toml)? else {
        return Ok(None);
    };
    Ok(Some(check_acceptance(&example_path, repo_key)?))
}

/// Record acceptance of `example_path`'s current bytes for `repo_key`, and
/// materialize the machine-local copy (byte-for-byte, at `machine_path` —
/// callers select the extension-matching sibling of the example: `.ts`
/// examples materialize to the `RUNTIME_CONFIG_SOURCE` path, `.toml`
/// examples to `RUNTIME_CONFIG`/`GLOBAL_RUNTIME_CONFIG`).
pub fn accept(
    example_path: &Utf8Path,
    machine_path: &Utf8Path,
    repo_key: &str,
) -> crate::Result<String> {
    let digest = crate::config_source::hash_file(example_path)?.to_string();
    let trait_id = acceptance_trait_id(repo_key);
    trust::update_named_digest(
        &trait_id,
        &digest,
        TrustState::Verified,
        Some(format!("accepted {example_path}")),
    )?;
    if let Some(parent) = machine_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    std::fs::copy(example_path, machine_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: machine_path.to_string(),
            source,
        }
    })?;
    stash_accepted_bytes(repo_key, example_path)?;
    Ok(digest)
}

fn stash_path(repo_key: &str) -> crate::Result<Utf8PathBuf> {
    Ok(crate::state::global_ctx_root()?
        .join("runtime-accepted")
        .join(format!("{repo_key}.example")))
}

/// Stash the just-accepted example's bytes so a later re-acceptance can
/// diff against them (the trust store records only a digest, per
/// [`accept`]'s doc comment). Best-effort sidecar: absence degrades the
/// diff to "digest changed, full new content shown" — never a hard error.
fn stash_accepted_bytes(repo_key: &str, example_path: &Utf8Path) -> crate::Result<()> {
    let path = stash_path(repo_key)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    std::fs::copy(example_path, &path).map_err(|source| crate::environment::Error::Filesystem {
        path: path.to_string(),
        source,
    })?;
    Ok(())
}

/// Read back the previously-accepted example's stashed bytes for
/// `repo_key`, if a stash exists (it may not, on stores predating this
/// stash or after manual cleanup).
pub fn stashed_bytes(repo_key: &str) -> crate::Result<Option<Vec<u8>>> {
    let path = stash_path(repo_key)?;
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|source| crate::environment::Error::Filesystem {
        path: path.to_string(),
        source,
    })?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn scratch_dir() -> Utf8PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "runtime-acceptance-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Utf8PathBuf::from_path_buf(dir).unwrap()
    }

    #[test]
    fn no_example_is_no_example() {
        let root = scratch_dir();
        let found = find_example(
            &root.join("runtime.example.ts"),
            &root.join("runtime.example.toml"),
        )
        .unwrap();
        assert_eq!(found, None);
    }

    #[test]
    fn both_examples_present_refuses() {
        let root = scratch_dir();
        std::fs::write(root.join("runtime.example.ts"), "").unwrap();
        std::fs::write(root.join("runtime.example.toml"), "").unwrap();
        let err = find_example(
            &root.join("runtime.example.ts"),
            &root.join("runtime.example.toml"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("one authority per fact"));
    }
}
