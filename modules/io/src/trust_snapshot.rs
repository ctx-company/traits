//! What a reviewer actually approved, kept so a later change can be shown
//! rather than merely announced.
//!
//! Approving a trait records a verdict against one canonical digest. Edit a
//! protected resource and the canonical digest moves, so the verdict no longer
//! applies and the trait reads as unreviewed — correct, and useless on its
//! own: "this is not the trait you approved" does not say what is different
//! about it, and a reviewer who cannot see the difference re-approves blind.
//!
//! So approval also snapshots the bytes: the canonical as approved, and every
//! protected resource's content. When the digest later moves, the two can be
//! diffed and the answer is a diff, not an assertion.
//!
//! Machine-local, beside the trust store it explains — a verdict is this
//! machine's, and so is the evidence for it. Content-addressed, so a resource
//! that appears in ten approvals is stored once and re-approving an unchanged
//! trait writes nothing.

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::Trait;
use ctx_traits_core::digest::Digest;

use crate::resource::ResourceRoots;

/// Root of the snapshot store: `<global>/traits/approved`.
fn store_root() -> crate::Result<Utf8PathBuf> {
    Ok(crate::state::global_trait_root()?.join("approved"))
}

/// Where one approved canonical's record lives. Keyed by the digest the
/// verdict was recorded against, which is exactly what the trust store keys
/// on — so a verdict and its evidence can never drift apart.
fn snapshot_path(canonical_digest: &str) -> crate::Result<Utf8PathBuf> {
    Ok(store_root()?
        .join("canonical")
        .join(format!("{}.toml", digest_filename(canonical_digest))))
}

/// Where one resource's bytes live, addressed by their own digest.
fn blob_path(resource_digest: &str) -> crate::Result<Utf8PathBuf> {
    Ok(store_root()?
        .join("blobs")
        .join(digest_filename(resource_digest)))
}

/// `sha256:<hex>` is not a filename on every filesystem; the hex alone is.
fn digest_filename(digest: &str) -> String {
    digest.rsplit(':').next().unwrap_or(digest).to_string()
}

/// Record what is being approved: the canonical text, and the bytes of every
/// protected resource it declares.
///
/// Best-effort by design. A snapshot that cannot be written must never fail an
/// approval — the verdict is the product, this is the explanation for a
/// question nobody has asked yet. A missing snapshot degrades to the behaviour
/// before it existed: the trait reads as unreviewed and says so without
/// showing why.
pub fn record_approval(
    canonical_digest: &str,
    canonical_text: &str,
    roots: &ResourceRoots,
    trait_ref: &Trait,
) {
    let Ok(path) = snapshot_path(canonical_digest) else {
        return;
    };
    if path.is_file() {
        // Same bytes, already stored. Re-approving an unchanged trait is
        // common (a second machine, a re-run) and should write nothing.
        return;
    }
    if write_atomic(&path, canonical_text.as_bytes()).is_err() {
        return;
    }
    for resource in &trait_ref.resources {
        if !resource.is_protected() {
            continue;
        }
        let Some(digest) = resource.digest.as_ref() else {
            continue;
        };
        let Ok(blob) = blob_path(digest.as_str()) else {
            continue;
        };
        if blob.is_file() {
            continue;
        }
        if let Ok(read) = crate::resource::read_text_resource(roots, resource)
            && let Some(text) = read.text.as_ref()
        {
            let _ = write_atomic(&blob, text.as_bytes());
        }
    }
}

/// One protected resource that differs from the approved snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceChange {
    pub resource_id: String,
    pub path: String,
    /// The content as approved. `None` when the resource is new since, or its
    /// blob was never stored.
    pub approved: Option<String>,
    /// The content now. `None` when the file is gone or unreadable.
    pub current: Option<String>,
}

/// Which protected resources differ between `approved_digest` and the trait as
/// it stands.
///
/// Empty when nothing differs, when no snapshot was kept, or when the change
/// was somewhere other than the resources — this answers "did a resource
/// change, and to what", not "why did the digest move". A caller that gets
/// nothing back should say the trait changed without claiming where.
pub fn resource_changes(
    approved_digest: &str,
    current: &Trait,
    roots: &ResourceRoots,
) -> Vec<ResourceChange> {
    let Some(approved) = read_approved_trait(approved_digest) else {
        return Vec::new();
    };
    let mut changes = Vec::new();
    for resource in &current.resources {
        if !resource.is_protected() {
            continue;
        }
        let Some(current_digest) = resource.digest.as_ref() else {
            continue;
        };
        let approved_resource = approved
            .resources
            .iter()
            .find(|candidate| candidate.id == resource.id);
        let approved_digest_value = approved_resource.and_then(|entry| entry.digest.as_ref());
        if approved_digest_value.map(Digest::as_str) == Some(current_digest.as_str()) {
            continue;
        }
        changes.push(ResourceChange {
            resource_id: resource.id.clone(),
            path: resource.path.clone().unwrap_or_default(),
            approved: approved_digest_value.and_then(|digest| read_blob(digest.as_str())),
            current: crate::resource::read_text_resource(roots, resource)
                .ok()
                .and_then(|read| read.text),
        });
    }
    changes
}

fn read_approved_trait(canonical_digest: &str) -> Option<Trait> {
    let path = snapshot_path(canonical_digest).ok()?;
    let text = std::fs::read_to_string(path.as_std_path()).ok()?;
    ctx_traits_core::encoding::decode_trait(ctx_traits_core::encoding::Encoding::Toml, &text).ok()
}

fn read_blob(resource_digest: &str) -> Option<String> {
    let path = blob_path(resource_digest).ok()?;
    std::fs::read_to_string(path.as_std_path()).ok()
}

/// Write through a temporary sibling so a reader never sees a half-written
/// snapshot, and a crash mid-write leaves the previous state rather than a
/// truncated one.
fn write_atomic(path: &Utf8Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent.as_std_path())?;
    }
    let temporary = path.with_extension("writing");
    std::fs::write(temporary.as_std_path(), bytes)?;
    std::fs::rename(temporary.as_std_path(), path.as_std_path())
}
