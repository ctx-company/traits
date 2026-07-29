//! Resource file digesting: read declared resources and compute digests.
//!
//! IO reads resource blobs under a trait package root, computes source
//! digests, and produces a manifest digest over all declared resources.
//! Security checks enforce path containment and symlink rejection so
//! resource declarations cannot escape the trait root.
//!
//! Path resolution is missing-file-safe and symlink-safe:
//! - Only the trait package root is canonicalized.
//! - The declared resource path is validated as a safe relative string,
//!   then joined to the canonical root.
//! - Each path component is walked with `symlink_metadata` before reading.
//!   Symlinks are reported and not followed; missing files are warnings,
//!   not errors.
//!
//! Binary resources are reported but still digested: a `BinaryContent`
//! warning is emitted alongside a `ResourceFileDigest` with `is_binary =
//! true`, because manifest-drift detection remains useful for binary blobs.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::digest::Digest;
use ctx_traits_core::r#trait::{Resource, ResourceRender, ResourceRoot, ResourceTrigger};

/// The trusted roots a declared resource path may resolve against.
///
/// `package_root` jails package-relative resources exactly as before.
/// `invocation_repo_root` is the Git repository root of the invocation
/// working directory (never the trait package location), discovered only
/// when at least one declared resource requires `root = "repo"`.
/// Dependency-qualified repo resources reuse the same invocation repo root;
/// package resources stay jailed per dependency package.
#[derive(Debug, Clone)]
pub struct ResourceRoots {
    pub package_root: Utf8PathBuf,
    pub invocation_repo_root: Option<Utf8PathBuf>,
}

impl ResourceRoots {
    /// Package-only roots: no `root = "repo"` resource is present or the
    /// caller has already established it does not need repo resolution.
    pub fn package_only(package_root: &Utf8Path) -> Self {
        Self {
            package_root: package_root.to_path_buf(),
            invocation_repo_root: None,
        }
    }

    /// Package roots plus a discovered (or absent) invocation repository
    /// root, for callers that already resolved Git context.
    pub fn with_invocation_repo(
        package_root: &Utf8Path,
        invocation_repo_root: Option<Utf8PathBuf>,
    ) -> Self {
        Self {
            package_root: package_root.to_path_buf(),
            invocation_repo_root,
        }
    }

    fn select(&self, resource: &Resource) -> crate::Result<&Utf8Path> {
        match resource.effective_root() {
            ResourceRoot::Package => Ok(self.package_root.as_path()),
            ResourceRoot::Repo => self.invocation_repo_root.as_deref().ok_or_else(|| {
                crate::environment::Error::Filesystem {
                    path: resource.id.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "resource declares root = \"repo\" but no invocation Git repository was discovered",
                    ),
                }
                .into()
            }),
        }
    }
}

/// Resolve the trusted roots for a resource set: discover the invocation
/// repository root only when at least one declared resource requires
/// `root = "repo"`, so package-only traits keep working outside Git.
pub fn resolve_resource_roots(
    package_root: &Utf8Path,
    resources: &[Resource],
) -> crate::Result<ResourceRoots> {
    if resources
        .iter()
        .any(|resource| resource.effective_root() == ResourceRoot::Repo)
    {
        let repo_root = crate::repository::discover_repo_root()?;
        Ok(ResourceRoots::with_invocation_repo(
            package_root,
            Some(repo_root),
        ))
    } else {
        Ok(ResourceRoots::package_only(package_root))
    }
}

/// Size of the prefix scanned for binary-content detection.
const BINARY_SCAN_BYTES: usize = 8192;

/// A computed digest for one resource file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceFileDigest {
    /// The declared resource ID.
    pub resource_id: String,
    /// The declared package-relative path.
    pub path: String,
    /// The source digest of the file bytes.
    pub digest: Digest,
    /// File size in bytes.
    pub byte_size: u64,
    /// Whether the content was flagged as binary-like.
    pub is_binary: bool,
}

/// Warning produced while reading a resource file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceReadWarning {
    /// The resource file or a path component is a symlink.
    SymlinkDetected { resource_id: String, path: String },
    /// The declared resource file does not exist.
    MissingFile { resource_id: String, path: String },
    /// The path exists but is not a regular file.
    SpecialFile { resource_id: String, path: String },
    /// The file content appears to be binary (null bytes detected).
    BinaryContent {
        resource_id: String,
        path: String,
        byte_size: u64,
    },
}

/// Why a resource was skipped for text audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceTextSkipReason {
    /// Extension is not an author-visible text extension.
    UnsupportedExtension,
    /// The resource is missing.
    Missing,
    /// A symlink was detected and not followed.
    Symlink,
    /// The path exists but is not a regular file.
    SpecialFile,
    /// Content appears binary or is not valid UTF-8 text.
    Binary,
    /// Repo-root reference with a non-inlining trigger: the content is
    /// repo-owned and never enters the model view, so it is excluded from
    /// the hidden-content text audit by policy.
    RepoRootReference,
}

/// Result of safely reading one text resource for hidden-content audit.
#[derive(Debug, Clone)]
pub struct TextReadOutput {
    /// The declared resource ID.
    pub resource_id: String,
    /// The declared package-relative path.
    pub path: String,
    /// Text content when safely readable and text-like.
    pub text: Option<String>,
    /// Why the resource was skipped, if it was not read as text.
    pub skipped: Option<ResourceTextSkipReason>,
    /// Warnings collected during the read attempt.
    pub warnings: Vec<ResourceReadWarning>,
}

/// The outcome of reading one declared resource.
///
/// Binary resources still produce a digest (with `is_binary = true`) because
/// manifest drift detection remains useful. Missing and symlinked resources
/// produce `digest: None`.
#[derive(Debug, Clone)]
pub struct ReadOutput {
    /// The declared resource ID.
    pub resource_id: String,
    /// The declared package-relative path.
    pub path: String,
    /// The computed file digest. `None` only for missing or symlinked
    /// resources.
    pub digest: Option<ResourceFileDigest>,
    /// Warnings collected while reading this resource.
    pub warnings: Vec<ResourceReadWarning>,
}

/// The complete resource manifest digest for one trait.
///
/// Combines per-resource file digests and declaration metadata in stable
/// order. This digest is separate from the canonical trait digest and the
/// model-visible digest — it captures only the resource manifest state.
#[derive(Debug, Clone)]
pub struct ResourceManifestDigest {
    /// The trait ID whose resources were digested.
    pub trait_id: String,
    /// Per-resource file digests in declaration order.
    pub file_digests: Vec<ResourceFileDigest>,
    /// The combined manifest digest over all declared resources.
    pub manifest_digest: Digest,
    /// Warnings collected across all resources.
    pub warnings: Vec<ResourceReadWarning>,
}

/// Whether a resource's presentation path is safe to open.
///
/// `Available` is the only status where the returned path may be handed to a
/// caller as an openable file reference. `Missing`, `Symlink`, and
/// `SpecialFile` carry the same lexically-safe path (for logging/diagnostics)
/// but that path must never be presented as something to open: a symlink at
/// the leaf or an intermediate component would be followed by the OS the
/// moment it is opened, defeating the no-follow containment check that
/// rejected it here, and a directory/FIFO/device is not something an agent
/// can safely read as file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationStatus {
    /// Every path component exists, none is a symlink, and the leaf is a
    /// regular file.
    Available,
    /// The declared resource path does not exist.
    Missing,
    /// A symlink was detected at the leaf or an intermediate component.
    Symlink,
    /// The leaf exists but is not a regular file (directory, FIFO, device).
    SpecialFile,
}

/// A resource's resolved presentation path, paired with whether it is safe
/// to present as an openable file reference.
#[derive(Debug, Clone)]
pub struct PresentedResource {
    /// The lexically-safe path: always contained within the trusted root,
    /// regardless of status.
    pub path: Utf8PathBuf,
    pub status: PresentationStatus,
}

/// Outcome of verifying a protected (pinned) resource's actual bytes.
#[derive(Debug, Clone)]
pub enum ProtectionVerification {
    /// The resource declares no `digest` pin; not protected, nothing to
    /// verify.
    Unprotected,
    /// Actual bytes matched the declared pin. Carries the verified absolute
    /// path, safe to open or spawn immediately by the caller that requested
    /// verification — no further containment/symlink check is required.
    Verified { path: Utf8PathBuf },
    /// Verification failed; the resource must not be used.
    Failed(ProtectionFailure),
}

/// Why a protected resource failed verification.
#[derive(Debug, Clone)]
pub enum ProtectionFailure {
    /// The file was read but its digest does not match the declared pin.
    Mismatch {
        resource_id: String,
        expected: Digest,
        actual: Digest,
    },
    /// The file could not be safely read at all (missing, symlink, special
    /// file, or an I/O error while reading).
    Unavailable { resource_id: String, reason: String },
}

impl ProtectionFailure {
    pub fn resource_id(&self) -> &str {
        match self {
            Self::Mismatch { resource_id, .. } | Self::Unavailable { resource_id, .. } => {
                resource_id
            }
        }
    }
}

impl std::fmt::Display for ProtectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mismatch {
                resource_id,
                expected,
                actual,
            } => write!(
                f,
                "protected resource {resource_id:?} bytes do not match its declared pin: expected {expected}, got {actual}"
            ),
            Self::Unavailable {
                resource_id,
                reason,
            } => write!(
                f,
                "protected resource {resource_id:?} is unavailable: {reason}"
            ),
        }
    }
}

/// Verify a declared resource's actual file bytes against its canonical
/// `digest` pin, immediately before the caller opens/embeds/spawns it.
///
/// This is the single verifier every protection-sensitive IO edge (frame
/// presentation, command spawn, `check` diagnostics) must call: it resolves
/// through the same authoritative path resolution
/// [`digest_resource`]/[`presentation_path`] use (containment, no-follow,
/// regular-file), reads the file exactly once, and compares its digest to
/// the declaration. An unprotected resource (no declared pin) is reported as
/// [`ProtectionVerification::Unprotected`] rather than verified — presence of
/// a pin is what makes a resource protected at all.
pub fn verify_protected_resource(
    roots: &ResourceRoots,
    resource: &Resource,
) -> crate::Result<ProtectionVerification> {
    let Some(expected) = resource.digest.as_ref() else {
        return Ok(ProtectionVerification::Unprotected);
    };
    // `validate_resources` in core rejects a digest pin on a pathless
    // resource, so this is unreachable for a decoded manifest; fail closed
    // rather than assert.
    let Some(path) = resource.path.as_deref() else {
        return Ok(ProtectionVerification::Failed(
            ProtectionFailure::Unavailable {
                resource_id: resource.id.clone(),
                reason: "resource has no declared path".to_string(),
            },
        ));
    };

    let mut warnings = Vec::new();
    let resolved = resolve_resource_file(roots, resource, path, &mut warnings)?;
    let Some(read_path) = resolved.available_path else {
        return Ok(ProtectionVerification::Failed(
            ProtectionFailure::Unavailable {
                resource_id: resource.id.clone(),
                reason: protection_unavailable_reason(resolved.status()),
            },
        ));
    };

    let bytes = match fs::read(&read_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Ok(ProtectionVerification::Failed(
                ProtectionFailure::Unavailable {
                    resource_id: resource.id.clone(),
                    reason: format!("read error: {e}"),
                },
            ));
        }
    };
    let actual = Digest::from_bytes(&bytes);
    if &actual != expected {
        return Ok(ProtectionVerification::Failed(
            ProtectionFailure::Mismatch {
                resource_id: resource.id.clone(),
                expected: expected.clone(),
                actual,
            },
        ));
    }

    Ok(ProtectionVerification::Verified { path: read_path })
}

fn protection_unavailable_reason(status: PresentationStatus) -> String {
    match status {
        PresentationStatus::Missing => "declared resource file does not exist".to_string(),
        PresentationStatus::Symlink => "a symlink was detected and rejected".to_string(),
        PresentationStatus::SpecialFile => "the path is not a regular file".to_string(),
        PresentationStatus::Available => "declared resource file is unavailable".to_string(),
    }
}

/// Resolve a declared resource's safe presentation path, without reading its
/// content.
///
/// Delegates to `resolve_resource_file`, the single authoritative resolution
/// used by `digest_resource`/`read_text_resource`/`presentation_path` alike,
/// so presentation can never diverge from digesting or text-reading on
/// symlink or special-file status. The returned path always stays within the
/// trusted root, but callers MUST check `status` before treating it as an
/// openable file: anything other than `Available` means the no-follow checks
/// rejected the path or it is not a regular file, and presenting it as
/// something to open would let the OS follow a symlink out of the declared
/// root, or hand back directory/device content the moment it is read.
pub fn presentation_path(
    roots: &ResourceRoots,
    resource: &Resource,
    relative_path: &str,
) -> crate::Result<PresentedResource> {
    let mut warnings = Vec::new();
    let resolved = resolve_resource_file(roots, resource, relative_path, &mut warnings)?;
    let status = resolved.status();
    Ok(PresentedResource {
        path: resolved.nominal_path,
        status,
    })
}

/// Read and digest one declared resource under a trait root.
///
/// Performs:
/// 1. Resolve the path through `resolve_resource_file`: select the trusted
///    root, validate and join the declared path, verify lexical containment,
///    walk each component with `symlink_metadata` (no-follow), and re-check
///    the leaf immediately before reading to guard against TOCTOU symlink or
///    special-file swaps.
/// 2. If not `Available`, return with no digest and the collected warnings.
/// 3. Otherwise read bytes, detect binary content, compute SHA-256 digest.
pub fn digest_resource(roots: &ResourceRoots, resource: &Resource) -> crate::Result<ReadOutput> {
    let Some(path) = resource.path.as_deref() else {
        return Err(crate::environment::Error::Filesystem {
            path: resource.id.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "inline resources must not be read through IO",
            ),
        }
        .into());
    };
    let mut warnings = Vec::new();

    let resolved = resolve_resource_file(roots, resource, path, &mut warnings)?;
    let Some(read_path) = resolved.available_path else {
        return Ok(ReadOutput {
            resource_id: resource.id.clone(),
            path: path.to_string(),
            digest: None,
            warnings,
        });
    };

    let bytes = match fs::read(&read_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warnings.push(ResourceReadWarning::MissingFile {
                resource_id: resource.id.clone(),
                path: path.to_string(),
            });
            return Ok(ReadOutput {
                resource_id: resource.id.clone(),
                path: path.to_string(),
                digest: None,
                warnings,
            });
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: read_path.to_string(),
                source: e,
            }
            .into());
        }
    };

    let byte_size = bytes.len() as u64;
    let is_binary = is_likely_binary(&bytes);

    if is_binary {
        warnings.push(ResourceReadWarning::BinaryContent {
            resource_id: resource.id.clone(),
            path: path.to_string(),
            byte_size,
        });
    }

    let digest = Digest::from_bytes(&bytes);

    Ok(ReadOutput {
        resource_id: resource.id.clone(),
        path: path.to_string(),
        digest: Some(ResourceFileDigest {
            resource_id: resource.id.clone(),
            path: path.to_string(),
            digest,
            byte_size,
            is_binary,
        }),
        warnings,
    })
}

/// Safely read a declared text resource for general inline rendering.
///
/// Uses the same containment and no-follow policy as resource digesting. Any
/// valid UTF-8 regular file is decoded regardless of extension — inline
/// presentation and template scanning are not restricted to author-visible
/// text extensions (that allowlist is an audit-only concern; see
/// [`read_text_resource_for_audit`]). Symlinks, special files, missing files,
/// binary content, and invalid UTF-8 are reported and skipped, never decoded
/// lossy.
pub fn read_text_resource(
    roots: &ResourceRoots,
    resource: &Resource,
) -> crate::Result<TextReadOutput> {
    read_text_resource_inner(roots, resource, false)
}

/// Safely read a declared text resource for the hidden-content audit.
///
/// Identical to [`read_text_resource`] except that only author-visible text
/// extensions are decoded — the audit deliberately scans just the files it
/// treats as human-authored text. Everything else is skipped as an unsupported
/// extension. The read/containment/UTF-8 safety logic is shared, so only the
/// extension-gate decision differs between the two entry points.
pub fn read_text_resource_for_audit(
    roots: &ResourceRoots,
    resource: &Resource,
) -> crate::Result<TextReadOutput> {
    read_text_resource_inner(roots, resource, true)
}

fn read_text_resource_inner(
    roots: &ResourceRoots,
    resource: &Resource,
    gate_audit_extension: bool,
) -> crate::Result<TextReadOutput> {
    let Some(path) = resource.path.as_deref() else {
        return Ok(TextReadOutput {
            resource_id: resource.id.clone(),
            path: String::new(),
            text: resource.content.clone(),
            skipped: None,
            warnings: Vec::new(),
        });
    };
    let mut warnings = Vec::new();

    if gate_audit_extension && !is_text_audit_extension(path) {
        return Ok(TextReadOutput {
            resource_id: resource.id.clone(),
            path: path.to_string(),
            text: None,
            skipped: Some(ResourceTextSkipReason::UnsupportedExtension),
            warnings,
        });
    }

    let resolved = resolve_resource_file(roots, resource, path, &mut warnings)?;
    let Some(read_path) = resolved.available_path else {
        let skipped = match resolved.status() {
            PresentationStatus::Symlink => ResourceTextSkipReason::Symlink,
            PresentationStatus::SpecialFile => ResourceTextSkipReason::SpecialFile,
            PresentationStatus::Missing | PresentationStatus::Available => {
                ResourceTextSkipReason::Missing
            }
        };
        return Ok(TextReadOutput {
            resource_id: resource.id.clone(),
            path: path.to_string(),
            text: None,
            skipped: Some(skipped),
            warnings,
        });
    };

    let bytes = match fs::read(&read_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warnings.push(ResourceReadWarning::MissingFile {
                resource_id: resource.id.clone(),
                path: path.to_string(),
            });
            return Ok(TextReadOutput {
                resource_id: resource.id.clone(),
                path: path.to_string(),
                text: None,
                skipped: Some(ResourceTextSkipReason::Missing),
                warnings,
            });
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: read_path.to_string(),
                source: e,
            }
            .into());
        }
    };

    if is_likely_binary(&bytes) {
        warnings.push(ResourceReadWarning::BinaryContent {
            resource_id: resource.id.clone(),
            path: path.to_string(),
            byte_size: bytes.len() as u64,
        });
        return Ok(TextReadOutput {
            resource_id: resource.id.clone(),
            path: path.to_string(),
            text: None,
            skipped: Some(ResourceTextSkipReason::Binary),
            warnings,
        });
    }

    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            warnings.push(ResourceReadWarning::BinaryContent {
                resource_id: resource.id.clone(),
                path: path.to_string(),
                byte_size: err.as_bytes().len() as u64,
            });
            return Ok(TextReadOutput {
                resource_id: resource.id.clone(),
                path: path.to_string(),
                text: None,
                skipped: Some(ResourceTextSkipReason::Binary),
                warnings,
            });
        }
    };

    Ok(TextReadOutput {
        resource_id: resource.id.clone(),
        path: path.to_string(),
        text: Some(text),
        skipped: None,
        warnings,
    })
}

/// Resource IDs whose bodies template-scan even when not on-activation:
/// declared template variants, resources with typed inputs, and any
/// resource transitively reachable from those through resource-kind inputs
/// or an unqualified prompt source reference.
pub fn resource_template_candidate_ids(
    trait_ref: &ctx_traits_core::Trait,
) -> std::collections::BTreeSet<String> {
    let resource_ids: std::collections::BTreeSet<&str> = trait_ref
        .resources
        .iter()
        .map(|resource| resource.id.as_str())
        .collect();
    let mut candidates = std::collections::BTreeSet::new();

    for resource in &trait_ref.resources {
        if resource.variant.as_deref() == Some("template") || !resource.input.is_empty() {
            candidates.insert(resource.id.clone());
        }
    }
    for (_, prompt) in trait_ref.prompts.iter() {
        if let Some(source) = prompt.source.as_deref() {
            if let Ok(parsed) = ctx_traits_core::reference::Reference::parse(source) {
                if parsed.kind() == ctx_traits_core::reference::Kind::Resource
                    && !parsed.is_qualified()
                    && resource_ids.contains(parsed.id())
                {
                    candidates.insert(parsed.id().to_string());
                }
            }
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for resource in &trait_ref.resources {
            if !candidates.contains(resource.id.as_str()) {
                continue;
            }
            for ref_text in resource.input.iter() {
                if let Ok(parsed) = ctx_traits_core::reference::Reference::parse(ref_text) {
                    if parsed.kind() == ctx_traits_core::reference::Kind::Resource
                        && !parsed.is_qualified()
                        && resource_ids.contains(parsed.id())
                        && candidates.insert(parsed.id().to_string())
                    {
                        changed = true;
                    }
                }
            }
        }
    }

    candidates
}

/// Read the bodies of resources that feed the model-view inline render:
/// any resource whose effective render mode is `inline`, plus any resource
/// reachable through [`resource_template_candidate_ids`]. This is
/// independent of `trigger` (the when-axis): an `on-demand` resource that
/// declares `render = "inline"` still needs its body scanned so inline
/// presentation has bytes to embed, and an `on-activation` reference-mode
/// resource does not need its bytes scanned merely because it activates
/// early.
pub fn scan_resource_bodies(
    roots: &ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<(
    Vec<ctx_traits_core::resource_plan::BodyEvidence>,
    Vec<ResourceReadWarning>,
)> {
    let candidate_ids = resource_template_candidate_ids(trait_ref);
    scan_resource_bodies_matching(roots, trait_ref, |resource| {
        // Inline bodies are already part of the compiled model view via the
        // pure-core render path; scanning them here too would double-count
        // the same content as separate body evidence.
        if resource.content.is_some() {
            return false;
        }
        candidate_ids.contains(resource.id.as_str())
            || resource.effective_render() == ResourceRender::Inline
    })
}

/// Read the bodies of every declared, readable resource regardless of
/// trigger — including on-demand resources that never enter the
/// model view. Dependency loading needs the full set so that a parent
/// trait's qualified reference to an on-demand dependency resource can be
/// materialized ([`crate::dependency::LoadedDependency::body_evidence`]) and
/// so hidden-content auditing covers every package resource, not just the
/// subset that feeds the dependency's own model-view render.
pub fn scan_all_resource_bodies(
    roots: &ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<(
    Vec<ctx_traits_core::resource_plan::BodyEvidence>,
    Vec<ResourceReadWarning>,
)> {
    scan_resource_bodies_matching(roots, trait_ref, |_resource| true)
}

fn scan_resource_bodies_matching(
    roots: &ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
    include: impl Fn(&Resource) -> bool,
) -> crate::Result<(
    Vec<ctx_traits_core::resource_plan::BodyEvidence>,
    Vec<ResourceReadWarning>,
)> {
    let mut evidence = Vec::new();
    let mut warnings = Vec::new();
    for resource in &trait_ref.resources {
        if !include(resource) {
            continue;
        }
        let outcome = read_text_resource(roots, resource)?;
        warnings.extend(outcome.warnings.clone());
        if let Some(text) = outcome.text.as_deref() {
            let (interps, mut diagnostics) =
                ctx_traits_core::r#trait::prompt::scan_interpolations(text);
            let mut detected_inputs = Vec::new();
            for interp in interps {
                match ctx_traits_core::reference::Reference::parse(&interp.ref_text) {
                    Ok(parsed)
                        if matches!(
                            parsed.kind(),
                            ctx_traits_core::reference::Kind::Port
                                | ctx_traits_core::reference::Kind::Slot
                                | ctx_traits_core::reference::Kind::Resource
                        ) =>
                    {
                        detected_inputs.push(interp.ref_text)
                    }
                    Ok(parsed) => diagnostics.push(format!(
                        "interpolation {{{}}} kind {:?} not allowed in resource template",
                        interp.ref_text,
                        parsed.kind()
                    )),
                    Err(_) => diagnostics.push(format!(
                        "interpolation {{{}}} is not a valid typed ref",
                        interp.ref_text
                    )),
                }
            }
            detected_inputs.sort();
            detected_inputs.dedup();
            evidence.push(ctx_traits_core::resource_plan::BodyEvidence {
                resource_id: resource.id.clone(),
                digest: Some(Digest::source(text)),
                text: Some(text.to_string()),
                detected_inputs,
                diagnostics,
            });
        }
    }
    Ok((evidence, warnings))
}

/// Read and digest all declared resources for a trait.
///
/// Resources are processed in declaration order. The manifest digest is
/// computed by combining declaration metadata and file digests in stable
/// (sorted by resource ID) order.
pub fn digest_resources(
    roots: &ResourceRoots,
    trait_id: &str,
    resources: &[Resource],
) -> crate::Result<ResourceManifestDigest> {
    let mut outcomes = Vec::with_capacity(resources.len());
    let mut all_warnings = Vec::new();

    for resource in resources {
        // Inline bodies are materialized by pure core and never receive host IO evidence.
        if resource.path.is_none() {
            continue;
        }
        let outcome = digest_resource(roots, resource)?;
        all_warnings.extend(outcome.warnings.clone());
        outcomes.push(outcome);
    }

    let mut file_digests: Vec<ResourceFileDigest> =
        outcomes.iter().filter_map(|o| o.digest.clone()).collect();

    file_digests.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));

    let manifest_digest = compute_manifest_digest(trait_id, resources, &file_digests);

    Ok(ResourceManifestDigest {
        trait_id: trait_id.to_string(),
        file_digests,
        manifest_digest,
        warnings: all_warnings,
    })
}

/// Compute a stable manifest digest over declared resources and their file
/// digests.
///
/// The manifest text is built by sorting resources by ID and concatenating
/// each resource's declaration metadata and file digest (or "missing") with
/// deterministic delimiters. The resulting bytes are SHA-256 hashed.
fn compute_manifest_digest(
    trait_id: &str,
    resources: &[Resource],
    file_digests: &[ResourceFileDigest],
) -> Digest {
    let digest_map: std::collections::BTreeMap<&str, &ResourceFileDigest> = file_digests
        .iter()
        .map(|fd| (fd.resource_id.as_str(), fd))
        .collect();

    let mut buf = String::new();
    buf.push_str("trait:");
    buf.push_str(trait_id);
    buf.push('\n');

    let mut sorted: Vec<&Resource> = resources.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    for resource in &sorted {
        buf.push_str("resource:");
        buf.push_str(&resource.id);
        buf.push('\n');
        if let Some(path) = resource.path.as_deref() {
            // Preserve the established file-backed preimage exactly.
            buf.push_str("  path:");
            buf.push_str(path);
        } else {
            buf.push_str("  content:");
            buf.push_str(Digest::source(resource.content.as_deref().unwrap_or_default()).as_str());
        }
        buf.push('\n');
        buf.push_str("  hint:");
        buf.push_str(resource.hint.as_deref().unwrap_or(""));
        buf.push('\n');
        buf.push_str("  trigger:");
        buf.push_str(format_trigger(resource.effective_trigger()));
        buf.push('\n');
        // Only repo-rooted entries get a discriminator: adding a package
        // marker unconditionally would change every existing manifest
        // digest, which must not happen.
        if resource.effective_root() == ResourceRoot::Repo {
            buf.push_str("  root:repo\n");
        }
        // Only an authored `render` changes the preimage: an omitted render
        // keeps the existing manifest digest stable, while switching a
        // declared path resource between reference and inline changes
        // evidence exactly as switching its body source would.
        if let Some(render) = resource.render {
            buf.push_str("  render:");
            buf.push_str(format_render(render));
            buf.push('\n');
        }
        match digest_map.get(resource.id.as_str()) {
            Some(fd) => {
                buf.push_str("  digest:");
                buf.push_str(fd.digest.as_str());
                buf.push('\n');
                buf.push_str("  binary:");
                buf.push_str(if fd.is_binary { "true" } else { "false" });
            }
            None => {
                buf.push_str("  digest:missing");
            }
        }
        buf.push('\n');
    }

    Digest::from_bytes(buf.as_bytes())
}

/// Canonicalize the trait root path and convert to UTF-8.
///
/// A built-in trait store package root gets one extra check first: a
/// no-follow walk (via [`crate::path_safety`]) over the exact
/// `<store-root>/<version>/<id>` chain (the active global
/// `.../cache/<repo-key>/builtin-traits/...` shape or the legacy
/// `.ctx/cache/builtin-traits/...` shape), so a symlinked version or id
/// directory is rejected before `canonicalize` would silently follow it.
/// Ordinary repo package roots are unaffected — this only runs when
/// [`crate::layout::is_builtin_store_package_root`] recognizes the shape.
fn canonicalize_utf8_root(trait_root: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    if crate::layout::is_builtin_store_package_root(trait_root) {
        crate::path_safety::ensure_no_symlink_ancestors(
            trait_root,
            "built-in trait store package root",
        )?;
    }
    let canonical =
        trait_root
            .canonicalize()
            .map_err(|e| crate::environment::Error::Filesystem {
                path: trait_root.to_string(),
                source: e,
            })?;
    Ok(Utf8PathBuf::from_path_buf(canonical).map_err(|_| {
        crate::environment::Error::Filesystem {
            path: trait_root.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonicalized trait root is not UTF-8",
            ),
        }
    })?)
}

/// Defensively validate a declared resource path in IO.
///
/// Duplicates core's path validation as a defense-in-depth check. Rejects
/// absolute paths, Windows drive prefixes, backslashes, `..` traversal, and
/// empty segments. Returns the validated relative path string.
fn validate_relative_resource_path(path: &str) -> crate::Result<String> {
    if path.is_empty() {
        return Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "resource path is empty"),
        }
        .into());
    }

    if path.starts_with('/') {
        return Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "resource path must be relative, not absolute",
            ),
        }
        .into());
    }

    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        return Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "resource path must be relative, not a Windows drive path",
            ),
        }
        .into());
    }

    if path.contains('\\') {
        return Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "resource path must use forward slashes, not backslashes",
            ),
        }
        .into());
    }

    for segment in path.split('/') {
        if segment == ".." {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "resource path must not contain '..'",
                ),
            }
            .into());
        }
        if segment.is_empty() {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "resource path must not contain empty segments",
                ),
            }
            .into());
        }
    }

    Ok(path.to_string())
}

/// The single authoritative resolution outcome for one declared resource
/// path: root selection, path validation, lexical containment, a no-follow
/// component walk, and a final leaf check that also classifies non-regular
/// files. `digest_resource`, `read_text_resource`, and `presentation_path`
/// all resolve through this one function, so a directory/FIFO/device can
/// never be `Available` on one path and `SpecialFile` on another.
struct ResolvedResourceFile {
    /// The lexically-safe nominal path: always contained within the trusted
    /// root, present regardless of outcome.
    nominal_path: Utf8PathBuf,
    /// The verified-safe path to open, present only when the leaf exists,
    /// is not a symlink, and is a regular file.
    available_path: Option<Utf8PathBuf>,
    is_symlink: bool,
    is_special_file: bool,
}

impl ResolvedResourceFile {
    fn status(&self) -> PresentationStatus {
        if self.available_path.is_some() {
            PresentationStatus::Available
        } else if self.is_symlink {
            PresentationStatus::Symlink
        } else if self.is_special_file {
            PresentationStatus::SpecialFile
        } else {
            PresentationStatus::Missing
        }
    }
}

/// Resolve one declared resource path against its trusted root.
///
/// Performs:
/// 1. Select the trusted root (package or invocation repo) for the
///    resource's declared `root`, then canonicalize only that root.
/// 2. Defensively validate the declared resource path (reject absolute,
///    drive-prefix, backslash, `..`, empty segments).
/// 3. Join to form the nominal path; verify lexical containment.
/// 4. Walk each path component with `symlink_metadata` (no-follow).
/// 5. Re-check the leaf's `symlink_metadata` immediately before returning it
///    as available, to catch TOCTOU races where the leaf is swapped to a
///    symlink or special file after the component walk, and to classify
///    non-regular leaves (directories, FIFOs, devices) as `SpecialFile`
///    rather than `Available`. This is best-effort within std; a
///    platform-supported no-follow open would be stronger.
fn resolve_resource_file(
    roots: &ResourceRoots,
    resource: &Resource,
    declared_path: &str,
    warnings: &mut Vec<ResourceReadWarning>,
) -> crate::Result<ResolvedResourceFile> {
    let canonical_root = canonicalize_utf8_root(roots.select(resource)?)?;
    let relative = validate_relative_resource_path(declared_path)?;
    let nominal = canonical_root.join(&relative);

    if !nominal.starts_with(&canonical_root) {
        return Err(crate::environment::Error::Filesystem {
            path: declared_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "resource path escapes the trait root",
            ),
        }
        .into());
    }

    let safe_path = inspect_path_without_following(
        &canonical_root,
        &relative,
        &resource.id,
        declared_path,
        warnings,
    )?;

    let Some(candidate) = safe_path else {
        let is_symlink = warnings
            .iter()
            .any(|warning| matches!(warning, ResourceReadWarning::SymlinkDetected { .. }));
        return Ok(ResolvedResourceFile {
            nominal_path: nominal,
            available_path: None,
            is_symlink,
            is_special_file: false,
        });
    };

    match fs::symlink_metadata(&candidate) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                warnings.push(ResourceReadWarning::SymlinkDetected {
                    resource_id: resource.id.clone(),
                    path: declared_path.to_string(),
                });
                return Ok(ResolvedResourceFile {
                    nominal_path: nominal,
                    available_path: None,
                    is_symlink: true,
                    is_special_file: false,
                });
            }
            if !meta.file_type().is_file() {
                warnings.push(ResourceReadWarning::SpecialFile {
                    resource_id: resource.id.clone(),
                    path: declared_path.to_string(),
                });
                return Ok(ResolvedResourceFile {
                    nominal_path: nominal,
                    available_path: None,
                    is_symlink: false,
                    is_special_file: true,
                });
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warnings.push(ResourceReadWarning::MissingFile {
                resource_id: resource.id.clone(),
                path: declared_path.to_string(),
            });
            return Ok(ResolvedResourceFile {
                nominal_path: nominal,
                available_path: None,
                is_symlink: false,
                is_special_file: false,
            });
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: candidate.to_string(),
                source: e,
            }
            .into());
        }
    }

    Ok(ResolvedResourceFile {
        nominal_path: nominal,
        available_path: Some(candidate),
        is_symlink: false,
        is_special_file: false,
    })
}

/// Walk each path component from the canonical root using
/// `symlink_metadata`.
///
/// Returns `Ok(None)` after pushing a warning if any component is a symlink
/// or is missing. Returns `Ok(Some(path))` only when all components exist
/// and none are symlinks.
fn inspect_path_without_following(
    canonical_root: &Utf8Path,
    relative_path: &str,
    resource_id: &str,
    declared_path: &str,
    warnings: &mut Vec<ResourceReadWarning>,
) -> crate::Result<Option<Utf8PathBuf>> {
    let mut current = canonical_root.to_owned();

    for segment in relative_path.split('/') {
        current = current.join(segment);

        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    warnings.push(ResourceReadWarning::SymlinkDetected {
                        resource_id: resource_id.to_string(),
                        path: declared_path.to_string(),
                    });
                    return Ok(None);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warnings.push(ResourceReadWarning::MissingFile {
                    resource_id: resource_id.to_string(),
                    path: declared_path.to_string(),
                });
                return Ok(None);
            }
            Err(e) => {
                return Err(crate::environment::Error::Filesystem {
                    path: current.to_string(),
                    source: e,
                }
                .into());
            }
        }
    }

    Ok(Some(current))
}

/// Heuristic binary-content check: true if the first `BINARY_SCAN_BYTES`
/// bytes contain a null byte.
fn is_likely_binary(bytes: &[u8]) -> bool {
    let scan_len = bytes.len().min(BINARY_SCAN_BYTES);
    bytes[..scan_len].contains(&0)
}

fn is_text_audit_extension(path: &str) -> bool {
    let extension = Utf8Path::new(path).extension().map(str::to_ascii_lowercase);
    matches!(
        extension.as_deref(),
        Some("md" | "markdown" | "txt" | "text" | "toml" | "json" | "yaml" | "yml")
    )
}

fn format_trigger(trigger: ResourceTrigger) -> &'static str {
    match trigger {
        ResourceTrigger::OnActivation => "on-activation",
        ResourceTrigger::OnDemand => "on-demand",
    }
}

fn format_render(render: ResourceRender) -> &'static str {
    match render {
        ResourceRender::Reference => "reference",
        ResourceRender::Inline => "inline",
    }
}
