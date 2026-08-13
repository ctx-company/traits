//! npm registry client (P438).
//!
//! Fetches package metadata and tarballs over HTTP entirely in Rust: no
//! `node`, `npm`, or `pnpm` process is ever spawned on this consume path.
//! Every tarball is verified against its published SHA-512 SRI integrity
//! before extraction, and extraction rejects absolute paths, traversal,
//! symlinks, hard links, special files, duplicate conflicting paths, and
//! oversized archives.

use std::collections::BTreeMap;
use std::io::Read;

use base64::Engine;
use camino::{Utf8Path, Utf8PathBuf};
use flate2::read::GzDecoder;
use sha2::{Digest as _, Sha512};

/// Default npm registry base URL.
pub const DEFAULT_REGISTRY_BASE: &str = "https://registry.npmjs.org";

const MAX_METADATA_BYTES: u64 = 20 * 1024 * 1024;
const MAX_TARBALL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_DISCOVERY_DEPTH: usize = 8;

/// Registry client failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("registry request to {url} failed: {message}")]
    Request { url: String, message: String },

    #[error("registry response for {url} was not valid JSON: {message}")]
    InvalidMetadata { url: String, message: String },

    #[error("package {package} has no published versions")]
    NoVersions { package: String },

    #[error(
        "package {package} version {version} publishes a tarball without sha512 integrity; refusing to install"
    )]
    MissingIntegrity { package: String, version: String },

    #[error("malformed integrity string {value:?} for {package}@{version}")]
    MalformedIntegrity {
        package: String,
        version: String,
        value: String,
    },

    #[error(
        "integrity mismatch for {package}@{version}: registry claims {expected}, computed {computed}"
    )]
    IntegrityMismatch {
        package: String,
        version: String,
        expected: String,
        computed: String,
    },

    #[error("{package}@{version} is not a valid gzip tar archive: {message}")]
    InvalidArchive {
        package: String,
        version: String,
        message: String,
    },

    #[error("unsafe tarball entry {path:?} in {package}@{version}: {reason}")]
    UnsafeEntry {
        package: String,
        version: String,
        path: String,
        reason: String,
    },

    #[error("{package}@{version} tarball has more than {limit} entries")]
    TooManyEntries {
        package: String,
        version: String,
        limit: usize,
    },

    #[error("{package}@{version} extracted content exceeds the {limit}-byte cap")]
    ExtractedTooLarge {
        package: String,
        version: String,
        limit: u64,
    },

    #[error("no trait package found inside {package}@{version}")]
    NoTraitPackageFound { package: String, version: String },

    #[error("malformed publisher ctx.digests claim in {package}: {message}")]
    MalformedClaim { package: String, message: String },

    #[error("vendored tree at {root} contains a symlink at {entry}, which is not permitted")]
    SymlinkInTree { root: String, entry: String },
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// npm package metadata document (the subset this client reads).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct RegistryMetadata {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "dist-tags", default)]
    pub dist_tags: BTreeMap<String, String>,
    #[serde(default)]
    pub versions: BTreeMap<String, RegistryVersion>,
}

impl RegistryMetadata {
    /// Every published version string, unordered.
    pub fn version_list(&self) -> Vec<String> {
        self.versions.keys().cloned().collect()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RegistryVersion {
    pub dist: RegistryDist,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RegistryDist {
    pub tarball: String,
    #[serde(default)]
    pub integrity: Option<String>,
}

/// Fetch package metadata from the registry.
pub fn fetch_metadata(base: &str, package: &str) -> Result<RegistryMetadata, Error> {
    let url = format!("{base}/{}", encode_package_path(package));
    let response = ureq::get(&url)
        .header("Accept", "application/json")
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|source| request_error(&url, &source))?;
    if !response.status().is_success() {
        return Err(Error::Request {
            url,
            message: format!("HTTP {}", response.status()),
        });
    }
    let bytes = response
        .into_body()
        .with_config()
        .limit(MAX_METADATA_BYTES)
        .read_to_vec()
        .map_err(|source| request_error(&url, &source))?;
    serde_json::from_slice(&bytes).map_err(|source| Error::InvalidMetadata {
        url,
        message: source.to_string(),
    })
}

fn encode_package_path(package: &str) -> String {
    package.replace('/', "%2f")
}

fn user_agent() -> String {
    format!("ctx-traits/{}", env!("CARGO_PKG_VERSION"))
}

fn request_error(url: &str, source: &ureq::Error) -> Error {
    Error::Request {
        url: url.to_string(),
        message: source.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tarball download and integrity verification
// ---------------------------------------------------------------------------

/// Download a tarball, bounded to [`MAX_TARBALL_BYTES`].
pub fn download_tarball(url: &str) -> Result<Vec<u8>, Error> {
    let response = ureq::get(url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|source| request_error(url, &source))?;
    if !response.status().is_success() {
        return Err(Error::Request {
            url: url.to_string(),
            message: format!("HTTP {}", response.status()),
        });
    }
    response
        .into_body()
        .with_config()
        .limit(MAX_TARBALL_BYTES)
        .read_to_vec()
        .map_err(|source| request_error(url, &source))
}

/// Verify tarball bytes against the registry's published SHA-512 SRI
/// integrity string. Packages without SHA-512 integrity are refused: this is
/// a hard requirement, not a warning, since it is the only pre-extraction
/// integrity evidence this client has.
pub fn verify_integrity(
    package: &str,
    version: &str,
    bytes: &[u8],
    integrity: Option<&str>,
) -> Result<(), Error> {
    let Some(integrity) = integrity else {
        return Err(Error::MissingIntegrity {
            package: package.to_string(),
            version: version.to_string(),
        });
    };
    let Some(encoded) = integrity.strip_prefix("sha512-") else {
        return Err(Error::MalformedIntegrity {
            package: package.to_string(),
            version: version.to_string(),
            value: integrity.to_string(),
        });
    };
    let expected = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| Error::MalformedIntegrity {
            package: package.to_string(),
            version: version.to_string(),
            value: integrity.to_string(),
        })?;
    let mut hasher = Sha512::new();
    hasher.update(bytes);
    let computed = hasher.finalize();
    if computed.as_slice() != expected.as_slice() {
        return Err(Error::IntegrityMismatch {
            package: package.to_string(),
            version: version.to_string(),
            expected: integrity.to_string(),
            computed: format!(
                "sha512-{}",
                base64::engine::general_purpose::STANDARD.encode(computed)
            ),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Safe extraction
// ---------------------------------------------------------------------------

/// Extract a verified tarball into a fresh staging directory.
///
/// `dest` must not already exist; it is created fresh. Rejects, per entry:
/// absolute paths, `..` traversal, symlinks, hard links, special files
/// (FIFOs/devices), and any path repeated by a later entry. Enforces a total
/// entry-count cap and a total extracted-byte cap (a decompression-bomb
/// guard). npm tarballs conventionally wrap all content under one `package/`
/// prefix, which is stripped when present; a tarball without that prefix
/// extracts its paths verbatim (still subject to every safety check above).
pub fn extract_verified_tarball(
    package: &str,
    version: &str,
    bytes: &[u8],
    dest: &Utf8Path,
) -> Result<(), Error> {
    extract_verified_tarball_with_prefix(package, version, bytes, dest, "package/")
}

/// Extract a verified tarball into a fresh staging directory, stripping
/// `strip_prefix` (rather than the npm-conventional `package/`) from every
/// entry path. Shared by the npm registry client
/// ([`extract_verified_tarball`], prefix `package/`) and
/// [`crate::git_fetch::fetch_codeload_snapshot`] (prefix
/// `<repo>-<sha>/`, GitHub codeload's tarball layout) so the two content
/// sources never diverge on the safety checks below.
pub fn extract_verified_tarball_with_prefix(
    package: &str,
    version: &str,
    bytes: &[u8],
    dest: &Utf8Path,
    strip_prefix: &str,
) -> Result<(), Error> {
    std::fs::create_dir_all(dest).map_err(|source| Error::InvalidArchive {
        package: package.to_string(),
        version: version.to_string(),
        message: format!("failed to create staging directory {dest}: {source}"),
    })?;

    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|source| Error::InvalidArchive {
        package: package.to_string(),
        version: version.to_string(),
        message: source.to_string(),
    })?;

    let mut seen_paths = std::collections::BTreeSet::new();
    let mut entry_count = 0usize;
    let mut extracted_bytes = 0u64;

    for entry in entries {
        let mut entry = entry.map_err(|source| Error::InvalidArchive {
            package: package.to_string(),
            version: version.to_string(),
            message: source.to_string(),
        })?;

        entry_count += 1;
        if entry_count > MAX_ENTRIES {
            return Err(Error::TooManyEntries {
                package: package.to_string(),
                version: version.to_string(),
                limit: MAX_ENTRIES,
            });
        }

        let raw_path = entry.path().map_err(|source| Error::InvalidArchive {
            package: package.to_string(),
            version: version.to_string(),
            message: source.to_string(),
        })?;
        let raw_path_str = raw_path.to_string_lossy().replace('\\', "/");
        let stripped = raw_path_str
            .strip_prefix(strip_prefix)
            .unwrap_or(&raw_path_str);

        let entry_type = entry.header().entry_type();
        if entry_type.is_pax_global_extensions() {
            // `git archive` (and therefore GitHub codeload) always emits a
            // leading `pax_global_header` ('g') entry carrying the commit
            // sha as a pax extension; tar-rs's non-raw entries iterator
            // already consumes GNU longname/longlink and pax *local* ('x')
            // headers on the caller's behalf but passes this one through
            // unconsumed, so every real codeload tarball hit `UnsafeEntry`
            // here without this arm. It carries no file content to extract.
            continue;
        }
        if entry_type.is_dir() {
            if stripped.is_empty() {
                continue;
            }
            let relative = safe_relative_path(package, version, stripped)?;
            std::fs::create_dir_all(dest.join(&relative)).map_err(|source| {
                Error::InvalidArchive {
                    package: package.to_string(),
                    version: version.to_string(),
                    message: source.to_string(),
                }
            })?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(Error::UnsafeEntry {
                package: package.to_string(),
                version: version.to_string(),
                path: stripped.to_string(),
                reason: format!("unsupported entry type {entry_type:?}"),
            });
        }
        if stripped.is_empty() {
            continue;
        }

        let relative = safe_relative_path(package, version, stripped)?;
        if !seen_paths.insert(relative.clone()) {
            return Err(Error::UnsafeEntry {
                package: package.to_string(),
                version: version.to_string(),
                path: relative.to_string(),
                reason: "duplicate path in archive".to_string(),
            });
        }

        extracted_bytes += entry.size();
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err(Error::ExtractedTooLarge {
                package: package.to_string(),
                version: version.to_string(),
                limit: MAX_EXTRACTED_BYTES,
            });
        }

        let target_path = dest.join(&relative);
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::InvalidArchive {
                package: package.to_string(),
                version: version.to_string(),
                message: source.to_string(),
            })?;
        }
        let mut contents = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut contents)
            .map_err(|source| Error::InvalidArchive {
                package: package.to_string(),
                version: version.to_string(),
                message: source.to_string(),
            })?;
        std::fs::write(&target_path, contents).map_err(|source| Error::InvalidArchive {
            package: package.to_string(),
            version: version.to_string(),
            message: format!("failed to write {target_path}: {source}"),
        })?;
    }

    Ok(())
}

fn safe_relative_path(package: &str, version: &str, path: &str) -> Result<Utf8PathBuf, Error> {
    let candidate = Utf8Path::new(path);
    if candidate.is_absolute() {
        return Err(Error::UnsafeEntry {
            package: package.to_string(),
            version: version.to_string(),
            path: path.to_string(),
            reason: "absolute path".to_string(),
        });
    }
    let mut normalized = Utf8PathBuf::new();
    for component in candidate.components() {
        match component {
            camino::Utf8Component::Normal(part) => normalized.push(part),
            camino::Utf8Component::CurDir => {}
            camino::Utf8Component::ParentDir => {
                return Err(Error::UnsafeEntry {
                    package: package.to_string(),
                    version: version.to_string(),
                    path: path.to_string(),
                    reason: "path traversal (..)".to_string(),
                });
            }
            camino::Utf8Component::RootDir | camino::Utf8Component::Prefix(_) => {
                return Err(Error::UnsafeEntry {
                    package: package.to_string(),
                    version: version.to_string(),
                    path: path.to_string(),
                    reason: "absolute path".to_string(),
                });
            }
        }
    }
    if normalized.as_str().is_empty() {
        return Err(Error::UnsafeEntry {
            package: package.to_string(),
            version: version.to_string(),
            path: path.to_string(),
            reason: "empty path".to_string(),
        });
    }
    Ok(normalized)
}

// ---------------------------------------------------------------------------
// Dual-use trait-package discovery
// ---------------------------------------------------------------------------

/// A discovered trait package root inside an extracted npm package, with its
/// path relative to the extraction root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPackage {
    pub relative_root: Utf8PathBuf,
    pub absolute_root: Utf8PathBuf,
}

/// Discover every trait package root inside an extracted npm package.
///
/// Walks the extracted tree looking for directories that resolve a canonical
/// or flat package manifest ([`crate::layout::resolve_package_manifest`]).
/// Does not assume the npm package root is itself a trait package: multi-
/// trait tarballs are supported by returning every discovered root. Skips
/// well-known non-package content directories (`node_modules`, `.git`, and
/// the package-internal `generated`/`source`/`resources`/`reference`/
/// `imported` directories) so their contents are never misidentified as
/// nested trait packages.
pub fn discover_trait_packages(root: &Utf8Path) -> Result<Vec<DiscoveredPackage>, Error> {
    let mut found = Vec::new();
    walk_for_packages(root, root, 0, &mut found);
    found.sort_by(|left, right| left.relative_root.cmp(&right.relative_root));
    Ok(found)
}

const SKIP_DIR_NAMES: &[&str] = &[
    "node_modules",
    ".git",
    "generated",
    "source",
    "resources",
    "reference",
    "imported",
];

fn walk_for_packages(
    root: &Utf8Path,
    dir: &Utf8Path,
    depth: usize,
    found: &mut Vec<DiscoveredPackage>,
) {
    if depth > MAX_DISCOVERY_DEPTH {
        return;
    }
    if crate::layout::resolve_package_manifest(dir).is_some() {
        let relative_root = dir.strip_prefix(root).unwrap_or(dir).to_path_buf();
        found.push(DiscoveredPackage {
            relative_root,
            absolute_root: dir.to_path_buf(),
        });
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<Utf8PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if SKIP_DIR_NAMES.contains(&name.as_str()) {
            continue;
        }
        if let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) {
            children.push(path);
        }
    }
    children.sort();
    for child in children {
        walk_for_packages(root, &child, depth + 1, found);
    }
}

/// Load the optional `ctx.digests` publisher claim from an extracted npm
/// package's root `package.json`.
pub fn load_publisher_claim(
    root: &Utf8Path,
) -> Result<Option<ctx_traits_core::distribution::PublisherDigestClaim>, Error> {
    let package_json = root.join("package.json");
    let Ok(text) = std::fs::read_to_string(&package_json) else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|source| Error::InvalidArchive {
            package: root.to_string(),
            version: String::new(),
            message: format!("invalid package.json: {source}"),
        })?;
    // Absent (no `ctx.digests` field at all) and malformed (present but not
    // the expected typed shape) are distinct outcomes: only the former may
    // downgrade to the no-claim warning path. A present-but-invalid claim
    // must fail before vendoring rather than silently discarding bad
    // entries and reporting itself as unclaimed.
    let Some(digests_value) = value.pointer("/ctx/digests") else {
        return Ok(None);
    };
    let Some(digests) = digests_value.as_object() else {
        return Err(Error::MalformedClaim {
            package: root.to_string(),
            message: "ctx.digests must be an object mapping canonical paths to sha256 digests"
                .to_string(),
        });
    };
    let mut map = BTreeMap::new();
    for (key, value) in digests {
        let Some(digest) = value.as_str() else {
            return Err(Error::MalformedClaim {
                package: root.to_string(),
                message: format!("ctx.digests.{key} must be a string digest"),
            });
        };
        if ctx_traits_core::digest::Digest::parse(digest).is_err() {
            return Err(Error::MalformedClaim {
                package: root.to_string(),
                message: format!("ctx.digests.{key} is not a valid sha256 digest: {digest:?}"),
            });
        }
        map.insert(key.clone(), digest.to_string());
    }
    Ok(Some(ctx_traits_core::distribution::PublisherDigestClaim {
        digests: map,
    }))
}

/// Repo-relative cache subdirectory shared by every registry-backed
/// resolution (project-level `ctx traits install` and package-local
/// `source.package` npm dependencies), so both consume exactly one verified,
/// collision-resistant cache rather than two separate directory families.
pub const REGISTRY_CACHE_SUBDIR: &str = "registry-packages";

/// Collision-resistant cache-directory key for one resolved npm package
/// version: a digest of the full `<package>@<version>` identity string, so
/// `@scope/name` and `scope__name` (or any other lossy human-readable
/// rewrite) can never collide, and a stale directory from a different
/// version can never be mistaken for the one just resolved.
fn cache_key(package: &str, version: &str) -> String {
    let digest =
        ctx_traits_core::digest::Digest::from_bytes(format!("{package}@{version}").as_bytes());
    digest.as_str().trim_start_matches("sha256:").to_string()
}

/// A resolved, extracted, integrity-verified npm package version.
pub struct FetchedPackage {
    /// Extraction root (deterministic cache directory; reused across calls
    /// for the same resolved version once its completion marker verifies).
    pub root: Utf8PathBuf,
    pub resolved_version: String,
    /// npm SRI integrity string for the tarball, e.g. `sha512-<base64>`.
    pub integrity: String,
}

/// Completion marker persisted alongside a cache entry so a later call can
/// tell "fully extracted and verified" apart from "partial, stale, or
/// tampered" without re-downloading: the registry's own SRI integrity for
/// this exact version, plus a full-tree content digest recomputed at reuse
/// time and compared against what was recorded right after verified
/// extraction.
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheCompletionMarker {
    integrity: String,
    tree_digest: String,
}

fn cache_marker_path(dest: &Utf8Path) -> Utf8PathBuf {
    dest.with_file_name(format!("{}.meta.json", dest.file_name().unwrap_or("pkg")))
}

/// `true` only when `path`'s own filesystem entry (never following a
/// symlink) is a directory: a symlink to a directory reports `false` here,
/// even though a following `is_dir()` would report `true`.
fn is_real_dir(path: &Utf8Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

/// `true` only when `path`'s own filesystem entry (never following a
/// symlink) is a regular file.
fn is_real_file(path: &Utf8Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

/// A cache entry is reusable only when `dest` is itself a real directory and
/// `marker_path` is itself a real regular file (never a symlink to either,
/// which could otherwise redirect reads outside the cache root), and their
/// recorded integrity/tree-digest evidence agrees with what is expected.
fn cache_entry_is_valid(dest: &Utf8Path, marker_path: &Utf8Path, expected_integrity: &str) -> bool {
    if !is_real_dir(dest) || !is_real_file(marker_path) {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(marker_path) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<CacheCompletionMarker>(&text) else {
        return false;
    };
    if marker.integrity != expected_integrity {
        return false;
    }
    matches!(compute_tree_digest(dest), Ok(digest) if digest == marker.tree_digest)
}

/// Discard a stale, corrupt, or hostile cache entry without ever following a
/// symlink at either `dest` or `marker_path`: a real directory is removed
/// with its contents, while any other leaf (regular file, symlink, or
/// special file) is unlinked directly so its target is never touched.
fn remove_cache_entry(dest: &Utf8Path, marker_path: &Utf8Path) -> crate::Result<()> {
    match std::fs::symlink_metadata(dest) {
        Ok(metadata) if metadata.is_dir() => {
            std::fs::remove_dir_all(dest).map_err(|source| {
                crate::environment::Error::Filesystem {
                    path: dest.to_string(),
                    source,
                }
            })?;
        }
        Ok(_) => {
            std::fs::remove_file(dest).map_err(|source| crate::environment::Error::Filesystem {
                path: dest.to_string(),
                source,
            })?;
        }
        Err(_) => {}
    }
    if std::fs::symlink_metadata(marker_path).is_ok() {
        std::fs::remove_file(marker_path).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: marker_path.to_string(),
                source,
            }
        })?;
    }
    Ok(())
}

/// Resolve `selector` against the registry, then download, verify, and
/// extract that exact resolved version's tarball into a deterministic,
/// collision-resistant, version-keyed directory under `cache_root`.
///
/// Reuses an already-extracted directory for the same resolved version
/// without a further network round-trip only when its completion marker's
/// recorded integrity matches the registry's current integrity for that
/// version *and* a freshly recomputed full-tree digest still matches the
/// digest recorded right after extraction; a missing, stale, mismatched, or
/// tampered cache entry is discarded and rebuilt through this same verified
/// path rather than trusted. Shared by both project-level
/// (`ctx traits install`) and package-local (`source.package` npm
/// dependency) npm resolution so neither ever falls back to invoking
/// `node`, `npm`, or `pnpm`.
pub fn fetch_and_extract_version(
    base: &str,
    package: &str,
    selector: &ctx_traits_core::distribution::VersionSelector,
    cache_root: &Utf8Path,
) -> crate::Result<FetchedPackage> {
    let metadata = fetch_metadata(base, package)?;
    if metadata.versions.is_empty() {
        return Err(Error::NoVersions {
            package: package.to_string(),
        }
        .into());
    }
    let versions = metadata.version_list();
    let resolved_version = ctx_traits_core::distribution::resolve_version(
        package,
        &versions,
        &metadata.dist_tags,
        selector,
    )
    .map_err(ctx_traits_core::Error::from)?;
    let version_entry =
        metadata
            .versions
            .get(&resolved_version)
            .ok_or_else(|| Error::NoVersions {
                package: package.to_string(),
            })?;
    let integrity =
        version_entry
            .dist
            .integrity
            .clone()
            .ok_or_else(|| Error::MissingIntegrity {
                package: package.to_string(),
                version: resolved_version.clone(),
            })?;

    let key = cache_key(package, &resolved_version);
    let dest = cache_root.join(&key);
    let marker_path = cache_marker_path(&dest);
    if cache_entry_is_valid(&dest, &marker_path, &integrity) {
        return Ok(FetchedPackage {
            root: dest,
            resolved_version,
            integrity,
        });
    }
    remove_cache_entry(&dest, &marker_path)?;

    let tarball_bytes = download_tarball(&version_entry.dist.tarball)?;
    verify_integrity(package, &resolved_version, &tarball_bytes, Some(&integrity))?;

    let staging = cache_root.join(format!("{key}.tmp-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: staging.to_string(),
                source,
            }
        })?;
    }
    extract_verified_tarball(package, &resolved_version, &tarball_bytes, &staging)?;
    let tree_digest = compute_tree_digest(&staging)?;

    if let Err(source) = std::fs::rename(&staging, &dest) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(crate::environment::Error::Filesystem {
            path: dest.to_string(),
            source,
        }
        .into());
    }
    let marker = CacheCompletionMarker {
        integrity: integrity.clone(),
        tree_digest,
    };
    let marker_text = serde_json::to_string(&marker).map_err(|source| Error::InvalidArchive {
        package: package.to_string(),
        version: resolved_version.clone(),
        message: format!("failed to encode cache completion marker: {source}"),
    })?;
    if let Err(err) = crate::project_lock::atomic_write_string(&marker_path, &marker_text) {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(err);
    }
    Ok(FetchedPackage {
        root: dest,
        resolved_version,
        integrity,
    })
}

/// Compute a deterministic aggregate digest over every regular file's
/// repository-relative path and content under `root`, sorted by path.
///
/// This is the authoritative "did anything in this tree change" evidence:
/// unlike a per-file digest map limited to files a caller already knows to
/// name, it also catches an added or removed file. Used both as the P438
/// project-lock's full-vendor-tree evidence (`PackageLockEntry::tree_digest`)
/// and as the registry cache's completion/tamper marker. Rejects (errors on)
/// any symlink found in the tree: neither a freshly extracted cache entry
/// nor a vendored package should ever contain one.
pub fn compute_tree_digest(root: &Utf8Path) -> crate::Result<String> {
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    entries.sort();
    let mut buffer = String::new();
    for (path, digest) in &entries {
        buffer.push_str(path);
        buffer.push('\0');
        buffer.push_str(digest);
        buffer.push('\n');
    }
    Ok(
        ctx_traits_core::digest::Digest::from_bytes(buffer.as_bytes())
            .as_str()
            .to_string(),
    )
}

fn collect_tree_entries(
    root: &Utf8Path,
    dir: &Utf8Path,
    out: &mut Vec<(String, String)>,
) -> crate::Result<()> {
    let read_dir =
        std::fs::read_dir(dir).map_err(|source| crate::environment::Error::Filesystem {
            path: dir.to_string(),
            source,
        })?;
    let mut children: Vec<Utf8PathBuf> = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: dir.to_string(),
            source,
        })?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|raw| {
            crate::environment::Error::Filesystem {
                path: raw.to_string_lossy().to_string(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF-8 path"),
            }
        })?;
        let file_type =
            entry
                .file_type()
                .map_err(|source| crate::environment::Error::Filesystem {
                    path: path.to_string(),
                    source,
                })?;
        if file_type.is_symlink() {
            return Err(Error::SymlinkInTree {
                root: root.to_string(),
                entry: path.to_string(),
            }
            .into());
        }
        if file_type.is_dir() {
            children.push(path);
        } else if file_type.is_file() {
            let bytes =
                std::fs::read(&path).map_err(|source| crate::environment::Error::Filesystem {
                    path: path.to_string(),
                    source,
                })?;
            let digest = ctx_traits_core::digest::Digest::from_bytes(&bytes);
            let relative = path.strip_prefix(root).unwrap_or(&path).to_string();
            out.push((relative, digest.as_str().to_string()));
        }
    }
    for child in children {
        collect_tree_entries(root, &child, out)?;
    }
    Ok(())
}
