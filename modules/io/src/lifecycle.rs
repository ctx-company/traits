//! Resolve a loaded trait's lifecycle status and machine trust verdict.
//!
//! Status and trust are owned by separate stores (Group 95, 2026-07-19):
//! status is the team-edited `[package].status` field in the package's
//! `trait.toml`; trust is this machine's verdict for the trait's canonical
//! digest in `~/.config/ctx/trust.toml`. The canonical trait document itself
//! carries neither field, so any caller that needs status/trust for gating,
//! reporting, or display must resolve it here rather than reading it off a
//! decoded `Trait`.

use camino::Utf8Path;
use ctx_traits_core::manifest::PackageStatus;
use ctx_traits_core::r#trait::TrustVerdict;

#[derive(Debug, Clone)]
pub struct StartAuthorization {
    pub status: PackageStatus,
    pub trust: TrustVerdict,
    pub approval: Option<crate::trust::TrustRecord>,
    pub decision: crate::trust::StartTrust,
}

/// Resolve `[package].status` for the package rooted at `trait_root`.
///
/// A missing manifest, or a manifest with no `[package]` table (a legacy
/// flat canonical document), defaults to `Draft` — the same default applied
/// to external imports per PRODUCT.md.
pub fn resolve_package_status(trait_root: &Utf8Path) -> crate::Result<PackageStatus> {
    let manifest_path = crate::layout::package_manifest_path(trait_root);
    if !manifest_path.is_file() {
        return Ok(PackageStatus::Draft);
    }
    let text = crate::read::read_text(&manifest_path)?;
    status_from_manifest_text(&text, manifest_path.as_str())
}

/// Decode `[package].status` from already-loaded manifest text (e.g. bytes
/// embedded for a built-in package, which has no on-disk `trait_root` to
/// read). A manifest with no `[package]` table (a legacy flat canonical
/// document) defaults to `Draft`, matching `resolve_package_status`.
fn status_from_manifest_text(text: &str, source_label: &str) -> crate::Result<PackageStatus> {
    let Some(manifest) = ctx_traits_core::manifest::decode_package_manifest(text, source_label)?
    else {
        return Ok(PackageStatus::Draft);
    };
    Ok(manifest.package.status)
}

/// Resolve package status and machine trust verdict for a built-in trait
/// package embedded in the binary. Built-ins have no on-disk `trait_root`
/// directory, so status is decoded from the embedded root `trait.toml` text
/// (falling back to `Draft` when absent, same as `resolve_package_status`)
/// and trust is looked up in the same machine trust store as local packages,
/// keyed by the built-in's own current canonical digest — the shared lookup
/// that both the local and built-in `ctx traits list` branches call so
/// neither duplicates digest computation or trust-store resolution.
pub fn resolve_builtin(
    package_manifest_text: Option<&str>,
    canonical_digest: &str,
) -> crate::Result<(PackageStatus, TrustVerdict)> {
    let status = match package_manifest_text {
        Some(text) => status_from_manifest_text(text, "trait.toml")?,
        None => PackageStatus::Draft,
    };
    Ok((status, resolve_trust_verdict(canonical_digest)?))
}

/// Resolve the machine trust verdict for a canonical digest.
///
/// No record for the digest means `Unreviewed`, never `Blocked`.
pub fn resolve_trust_verdict(canonical_digest: &str) -> crate::Result<TrustVerdict> {
    let store = crate::trust::read_store()?;
    Ok(match store.record(canonical_digest) {
        Some(record) => match record.state {
            crate::trust::TrustState::Verified => TrustVerdict::Verified,
            crate::trust::TrustState::Blocked => TrustVerdict::Blocked,
        },
        None => TrustVerdict::Unreviewed,
    })
}

/// Resolve trust against an identity-bound approval lineage. This is the
/// required surface for trait-aware callers; digest-only lookup remains only
/// for raw script decisions and legacy compatibility.
pub fn resolve_trust_verdict_for_trait(
    trait_id: &str,
    canonical_digest: &str,
) -> crate::Result<TrustVerdict> {
    let store = crate::trust::read_store()?;
    Ok(match store.start_trust(trait_id, canonical_digest) {
        crate::trust::StartTrust::Verified(_) => TrustVerdict::Verified,
        crate::trust::StartTrust::Blocked(_) => TrustVerdict::Blocked,
        crate::trust::StartTrust::Unreviewed => TrustVerdict::Unreviewed,
    })
}

/// Resolve both package status and machine trust verdict for a loaded trait.
pub fn resolve(
    trait_root: &Utf8Path,
    canonical_digest: &str,
) -> crate::Result<(PackageStatus, TrustVerdict)> {
    Ok((
        resolve_package_status(trait_root)?,
        resolve_trust_verdict(canonical_digest)?,
    ))
}

pub fn resolve_named(
    trait_root: &Utf8Path,
    trait_id: &str,
    canonical_digest: &str,
) -> crate::Result<(PackageStatus, TrustVerdict)> {
    Ok((
        resolve_package_status(trait_root)?,
        resolve_trust_verdict_for_trait(trait_id, canonical_digest)?,
    ))
}

/// Read package lifecycle and trust history once at the new-session boundary.
pub fn authorize_start(
    trait_root: &Utf8Path,
    trait_id: &str,
    canonical_digest: &str,
) -> crate::Result<StartAuthorization> {
    let status = resolve_package_status(trait_root)?;
    let decision =
        crate::trust::read_store_with_sequences()?.start_trust(trait_id, canonical_digest);
    let (trust, approval) = match &decision {
        crate::trust::StartTrust::Verified(record) => {
            (TrustVerdict::Verified, Some(record.clone()))
        }
        crate::trust::StartTrust::Blocked(_) => (TrustVerdict::Blocked, None),
        crate::trust::StartTrust::Unreviewed => (TrustVerdict::Unreviewed, None),
    };
    Ok(StartAuthorization {
        status,
        trust,
        approval,
        decision,
    })
}

/// Set `[package].status` in the package manifest rooted at `trait_root`.
///
/// This is the sole write path for `ctx traits activate`/`deactivate`
/// (Group 95, 2026-07-19): status lives only in the package manifest, never
/// on the canonical trait document. Errors if the package has no
/// `trait.toml` with a `[package]` table — a legacy flat canonical document
/// has no status field to edit.
pub fn write_package_status(trait_root: &Utf8Path, status: PackageStatus) -> crate::Result<()> {
    let manifest_path = crate::layout::package_manifest_path(trait_root);
    if !manifest_path.is_file() {
        return Err(crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "package has no trait.toml package manifest to hold [package].status",
            ),
        }
        .into());
    }
    let text = crate::read::read_text(&manifest_path)?;
    if ctx_traits_core::manifest::decode_package_manifest(&text, manifest_path.as_str())?.is_none()
    {
        return Err(crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "trait.toml has no [package] table; it is a legacy flat canonical document with no status field",
            ),
        }
        .into());
    }
    let new_text = set_package_status_line(&text, status);
    let Some(patched) =
        ctx_traits_core::manifest::decode_package_manifest(&new_text, manifest_path.as_str())?
    else {
        return Err(crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "status edit produced an unparseable trait.toml; refusing to write",
            ),
        }
        .into());
    };
    if patched.package.status != status {
        return Err(crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "status edit did not take effect; refusing to write",
            ),
        }
        .into());
    }
    crate::write::write_package_manifest(&manifest_path, &new_text)
}

/// Rewrite `status = "..."` inside the `[package]` table, preserving every
/// other byte of the file (comments, formatting, dependency tables) rather
/// than reserializing the whole manifest. Inserts a new `status` line right
/// after the `[package]` header if the table has no existing status line.
fn set_package_status_line(text: &str, status: PackageStatus) -> String {
    let new_line = format!("status = \"{}\"", status.as_str());
    let mut out: Vec<String> = Vec::new();
    let mut in_package_table = false;
    let mut wrote_status = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_package_table && !wrote_status {
                out.push(new_line.clone());
                wrote_status = true;
            }
            in_package_table = trimmed == "[package]";
            out.push(line.to_string());
            continue;
        }
        if in_package_table && trimmed.starts_with("status") && trimmed.contains('=') {
            out.push(new_line.clone());
            wrote_status = true;
            continue;
        }
        out.push(line.to_string());
    }
    if in_package_table && !wrote_status {
        out.push(new_line.clone());
    }
    let mut result = out.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    result
}
