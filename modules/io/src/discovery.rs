//! Project manifest discovery.
//!
//! Scans a repository root for `.ctx/traits.toml`, `.ctx/traits.json`,
//! `.ctx/traits.yaml`, and `.ctx/traits.yml` with deterministic conflict
//! diagnostics. Scanning is shallow and explicit: only the supplied root
//! directory is checked, no recursive walking.
//!
//! Also discovers portable protocol packages under the repo-local protocol root.

use camino::{Utf8Path, Utf8PathBuf};

/// Manifest file names in deterministic priority order.
const MANIFEST_FILES: &[(&str, &str)] = &[
    ("toml", "toml"),
    ("json", "json"),
    ("yaml", "yaml"),
    ("yml", "yml"),
];

/// Discovered manifest path and its encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub path: Utf8PathBuf,
    pub encoding: &'static str,
}

/// Outcome of manifest discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestDiscovery {
    /// Exactly one manifest found.
    Found(Manifest),
    /// No manifest found in the repository root.
    NotFound,
    /// Multiple manifest encodings found; explicit path required.
    Conflict { found: Vec<Manifest> },
}

/// Discover a project manifest in the given repository root.
///
/// Scans only `.ctx` for `traits.{toml,json,yaml,yml}`. If
/// multiple encodings exist, returns `Conflict` so the caller can report the
/// ambiguity and request an explicit `--manifest` path.
pub fn manifest(repo_root: &Utf8Path) -> Result<ManifestDiscovery, crate::Error> {
    let found = scan_manifest_files(repo_root)?;

    match found.len() {
        0 => Ok(ManifestDiscovery::NotFound),
        1 => {
            let mut found = found.into_iter();
            if let Some(manifest) = found.next() {
                Ok(ManifestDiscovery::Found(manifest))
            } else {
                Ok(ManifestDiscovery::NotFound)
            }
        }
        _ => Ok(ManifestDiscovery::Conflict { found }),
    }
}

/// Scan `.ctx` for manifest files in deterministic order.
fn scan_manifest_files(repo_root: &Utf8Path) -> Result<Vec<Manifest>, crate::Error> {
    let mut found = Vec::new();

    for (extension, encoding) in MANIFEST_FILES {
        let path = crate::layout::project_manifest_path(repo_root, extension);
        if path.is_file() {
            found.push(Manifest { path, encoding });
        }
    }

    Ok(found)
}

// ---------------------------------------------------------------------------
// Trait package discovery
// ---------------------------------------------------------------------------

/// A discovered trait package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitPackage {
    pub trait_id: String,
    pub trait_path: Utf8PathBuf,
}

/// One canonical manifest selected for package-local operations. Native
/// families contribute one row per declared leaf; ordinary packages retain
/// their root manifest and source-less packages remain discoverable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitPackageLeaf {
    pub trait_id: String,
    pub variant: Option<String>,
    pub trait_path: Utf8PathBuf,
}

/// A CDK authoring package discovered under `.ctx/traits`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitAuthoringPackage {
    pub trait_id: String,
    pub source_path: Utf8PathBuf,
}

/// Discover CDK authoring packages without exposing their paths as canonical
/// protocol manifests. The vendor directory is not a local trait inventory.
pub fn trait_authoring_packages(
    repo_root: &Utf8Path,
) -> Result<Vec<TraitAuthoringPackage>, crate::Error> {
    let authoring_root = crate::layout::trait_authoring_root_path(repo_root);
    let entries = match std::fs::read_dir(&authoring_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: authoring_root.to_string(),
                source: e,
            }
            .into());
        }
    };

    let mut packages = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| crate::environment::Error::Filesystem {
            path: authoring_root.to_string(),
            source: e,
        })?;
        if !entry
            .file_type()
            .map_err(|e| crate::environment::Error::Filesystem {
                path: authoring_root.to_string(),
                source: e,
            })?
            .is_dir()
        {
            continue;
        }
        let Some(trait_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if trait_id == "vendor" {
            continue;
        }
        if let Some(source_path) = crate::layout::trait_cdk_source_path(repo_root, &trait_id)? {
            packages.push(TraitAuthoringPackage {
                trait_id,
                source_path,
            });
        }
    }
    packages.sort_by(|left, right| left.trait_id.cmp(&right.trait_id));
    Ok(packages)
}

/// Discover the merged local trait inventory by ID. Protocol consumers must
/// continue to use [`trait_packages`] because source-only packages lack TOML.
pub fn trait_inventory_ids(repo_root: &Utf8Path) -> Result<Vec<String>, crate::Error> {
    let mut ids: std::collections::BTreeSet<String> = trait_packages(repo_root)?
        .into_iter()
        .map(|package| package.trait_id)
        .collect();
    ids.extend(
        trait_authoring_packages(repo_root)?
            .into_iter()
            .map(|package| package.trait_id),
    );
    Ok(ids.into_iter().collect())
}

/// Discover portable protocol packages under the repo-local protocol root.
///
/// Scans one level deep: each subdirectory of the source root is treated as
/// a candidate trait package. Only `trait.toml` is checked (the preferred
/// encoding). Returns results in deterministic sorted order by trait ID.
pub fn trait_packages(repo_root: &Utf8Path) -> Result<Vec<TraitPackage>, crate::Error> {
    let traits_root = crate::layout::trait_protocol_root_path(repo_root);
    let mut packages = Vec::new();

    for name in trait_package_dir_names(&traits_root)? {
        let trait_path = crate::layout::trait_manifest_path(repo_root, &name)?;
        if trait_path.is_file() {
            packages.push(TraitPackage {
                trait_id: name,
                trait_path,
            });
        }
    }

    Ok(packages)
}

fn trait_package_dir_names(traits_root: &Utf8Path) -> Result<Vec<String>, crate::Error> {
    let entries = match std::fs::read_dir(traits_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: traits_root.to_string(),
                source: e,
            }
            .into());
        }
    };

    let mut dir_names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| crate::environment::Error::Filesystem {
            path: traits_root.to_string(),
            source: e,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|e| crate::environment::Error::Filesystem {
                path: traits_root.to_string(),
                source: e,
            })?;
        if file_type.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            dir_names.push(name.to_string());
        }
    }

    dir_names.sort();

    dir_names.retain(|name| name != "vendor");
    Ok(dir_names)
}

/// Expand every discovered package root to the canonical manifests operated
/// on by vendor/check evidence consumers.
pub fn trait_package_leaves(repo_root: &Utf8Path) -> Result<Vec<TraitPackageLeaf>, crate::Error> {
    let mut leaves = Vec::new();
    let traits_root = crate::layout::trait_protocol_root_path(repo_root);
    for trait_id in trait_package_dir_names(&traits_root)? {
        let root = traits_root.join(&trait_id);
        let package_manifest = crate::layout::package_manifest_path(&root);
        match crate::family_manifest::read_family_table(&package_manifest)? {
            Some(family) => {
                for (variant, leaf) in family.leaves {
                    leaves.push(TraitPackageLeaf {
                        trait_id: trait_id.clone(),
                        variant: Some(variant),
                        trait_path: root.join(leaf.relative_path),
                    });
                }
            }
            None => {
                let trait_path = crate::layout::trait_manifest_path(repo_root, &trait_id)?;
                if trait_path.is_file() {
                    leaves.push(TraitPackageLeaf {
                        trait_id,
                        variant: None,
                        trait_path,
                    });
                }
            }
        }
    }
    leaves.sort_by(|left, right| {
        left.trait_id
            .cmp(&right.trait_id)
            .then_with(|| left.variant.cmp(&right.variant))
    });
    Ok(leaves)
}
