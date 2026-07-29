use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
use ctx_traits_core::import::plan::{ImportProfile, ImportReport, ManagedImportArtifact};

use super::support::{
    digest_entries, digest_regular_import_file, filesystem_error, is_safe_relative_path,
    read_regular_utf8_file, reject_existing_symlink_ancestors, relative_import_path,
    validate_generated_leaf, validate_path_shape,
};

/// A loaded Agent Skills-compatible source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedAgentSkillSource {
    /// Source root used for raw preservation.
    pub source_root: Utf8PathBuf,
    /// Concrete SKILL.md path read for conversion.
    pub skill_path: Utf8PathBuf,
    /// Stable source name used as an ID/name fallback.
    pub source_name: String,
    /// UTF-8 Markdown content from SKILL.md.
    pub skill_markdown: String,
}

/// A planned copy of one raw imported file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCopyEntry {
    /// Source path (in the original source directory).
    pub source: String,
    /// Destination path under `imported/` in the trait package.
    pub destination: String,
    /// Whether a file already exists at the destination.
    pub conflict: bool,
}

/// A warning about the import copy plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportCopyWarning {
    /// A file at the destination already exists.
    PathConflict { destination: String },
    /// The source layout has unsafe or unexpected paths.
    UnsafeSourceLayout { path: String },
}

/// The result of planning an import copy operation.
#[derive(Debug, Clone)]
pub struct ImportCopyPlan {
    /// Planned copy entries.
    pub entries: Vec<ImportCopyEntry>,
    /// Warnings about the plan.
    pub warnings: Vec<ImportCopyWarning>,
}

/// Result of writing an imported canonical package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWriteResult {
    pub trait_path: String,
    pub report_path: String,
    pub copied_entries: usize,
    pub managed_overwrite: bool,
}

/// Read a single Agent Skills `SKILL.md` from either a file path or a source
/// directory containing `SKILL.md`. Symlinks and special files are rejected.
pub fn read_agent_skill_source(source: &Utf8Path) -> crate::Result<LoadedAgentSkillSource> {
    let metadata =
        fs::symlink_metadata(source).map_err(|e| crate::environment::Error::Filesystem {
            path: source.to_string(),
            source: e,
        })?;

    if metadata.file_type().is_symlink() {
        return filesystem_error(source, "import source must not be a symlink");
    }

    if metadata.file_type().is_dir() {
        let skill_path = source.join("SKILL.md");
        let skill_markdown = read_regular_utf8_file(&skill_path)?;
        let source_name = source
            .file_name()
            .map_or("imported-skill", |name| name)
            .to_string();
        return Ok(LoadedAgentSkillSource {
            source_root: source.to_path_buf(),
            skill_path,
            source_name,
            skill_markdown,
        });
    }

    if metadata.file_type().is_file() {
        if source.file_name() != Some("SKILL.md") {
            return filesystem_error(source, "import source file must be named SKILL.md");
        }
        let skill_markdown = read_regular_utf8_file(source)?;
        let source_root = source
            .parent()
            .map_or_else(|| Utf8PathBuf::from("."), Utf8Path::to_path_buf);
        let source_name = source_root
            .file_name()
            .or_else(|| source.file_stem())
            .map_or("imported-skill", |name| name)
            .to_string();
        return Ok(LoadedAgentSkillSource {
            source_root,
            skill_path: source.to_path_buf(),
            source_name,
            skill_markdown,
        });
    }

    filesystem_error(source, "import source must be a SKILL.md file or directory")
}

/// Plan the copy of raw source files into `imported/` under a trait package
/// root.
///
/// Scans the source directory for files and plans their copy into the
/// `imported/` subdirectory of the trait package. Does not execute copies.
/// Reports conflicts where destination files already exist.
pub fn plan_import_copy(
    trait_root: &Utf8Path,
    source_dir: &Utf8Path,
) -> crate::Result<ImportCopyPlan> {
    let imported_dir = trait_root.join("imported");
    let mut entries = Vec::new();
    let mut warnings = Vec::new();

    let source_metadata = match fs::symlink_metadata(source_dir) {
        Ok(metadata) => metadata,
        Err(_) => {
            warnings.push(ImportCopyWarning::UnsafeSourceLayout {
                path: source_dir.to_string(),
            });
            return Ok(ImportCopyPlan { entries, warnings });
        }
    };

    if source_metadata.file_type().is_symlink() {
        warnings.push(ImportCopyWarning::UnsafeSourceLayout {
            path: source_dir.to_string(),
        });
        return Ok(ImportCopyPlan { entries, warnings });
    }

    if source_metadata.file_type().is_file() {
        plan_single_import_file(source_dir, &imported_dir, &mut entries, &mut warnings);
        return sorted_copy_plan(entries, warnings);
    }

    if !source_metadata.file_type().is_dir() {
        warnings.push(ImportCopyWarning::UnsafeSourceLayout {
            path: source_dir.to_string(),
        });
        return Ok(ImportCopyPlan { entries, warnings });
    }

    match fs::symlink_metadata(&imported_dir) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                warnings.push(ImportCopyWarning::UnsafeSourceLayout {
                    path: imported_dir.to_string(),
                });
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => warnings.push(ImportCopyWarning::UnsafeSourceLayout {
            path: imported_dir.to_string(),
        }),
    }

    scan_directory(
        source_dir,
        source_dir,
        &imported_dir,
        &mut entries,
        &mut warnings,
    )?;

    sorted_copy_plan(entries, warnings)
}

fn sorted_copy_plan(
    mut entries: Vec<ImportCopyEntry>,
    mut warnings: Vec<ImportCopyWarning>,
) -> crate::Result<ImportCopyPlan> {
    entries.sort_by(|a, b| a.destination.cmp(&b.destination));
    warnings.sort_by(|a, b| match (a, b) {
        (
            ImportCopyWarning::PathConflict { destination: da },
            ImportCopyWarning::PathConflict { destination: db },
        ) => da.cmp(db),
        (
            ImportCopyWarning::UnsafeSourceLayout { path: pa },
            ImportCopyWarning::UnsafeSourceLayout { path: pb },
        ) => pa.cmp(pb),
        (ImportCopyWarning::PathConflict { .. }, ImportCopyWarning::UnsafeSourceLayout { .. }) => {
            std::cmp::Ordering::Less
        }
        (ImportCopyWarning::UnsafeSourceLayout { .. }, ImportCopyWarning::PathConflict { .. }) => {
            std::cmp::Ordering::Greater
        }
    });

    Ok(ImportCopyPlan { entries, warnings })
}

fn plan_single_import_file(
    source_file: &Utf8Path,
    imported_dir: &Utf8Path,
    entries: &mut Vec<ImportCopyEntry>,
    warnings: &mut Vec<ImportCopyWarning>,
) {
    let Some(file_name) = source_file.file_name() else {
        warnings.push(ImportCopyWarning::UnsafeSourceLayout {
            path: source_file.to_string(),
        });
        return;
    };
    let destination = imported_dir.join(file_name).to_string();
    let conflict = Utf8Path::new(&destination).exists();
    if conflict {
        warnings.push(ImportCopyWarning::PathConflict {
            destination: destination.clone(),
        });
    }
    entries.push(ImportCopyEntry {
        source: source_file.to_string(),
        destination,
        conflict,
    });
}

/// Validate and write an imported trait package, including raw source copies and
/// a generated `import-report.json` evidence file.
pub fn write_import_package(
    trait_path: &Utf8Path,
    trait_text: &str,
    report_json: &str,
    copy_plan: &ImportCopyPlan,
) -> crate::Result<ImportWriteResult> {
    let parent = trait_path
        .parent()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: trait_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "import target has no parent directory",
            ),
        })?;
    let package_root = crate::layout::package_root_for_manifest(trait_path).ok_or_else(|| {
        crate::environment::Error::Filesystem {
            path: trait_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "import target has no package root",
            ),
        }
    })?;
    validate_path_shape(trait_path)?;
    reject_existing_symlink_ancestors(parent)?;
    let managed_overwrite = validate_import_trait_target(trait_path)?;

    if !managed_overwrite
        && copy_plan
            .warnings
            .iter()
            .any(|warning| matches!(warning, ImportCopyWarning::PathConflict { .. }))
    {
        return filesystem_error(
            trait_path,
            "raw import preservation has path conflicts; refusing to create canonical trait",
        );
    }
    if copy_plan
        .warnings
        .iter()
        .any(|warning| matches!(warning, ImportCopyWarning::UnsafeSourceLayout { .. }))
    {
        return filesystem_error(
            trait_path,
            "raw import preservation has unsafe source layout warnings; refusing to create canonical trait",
        );
    }

    std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
        path: parent.to_string(),
        source: e,
    })?;
    reject_existing_symlink_ancestors(parent)?;

    for entry in &copy_plan.entries {
        copy_import_entry(entry, managed_overwrite)?;
    }

    let report_path = package_root.join("import-report.json");
    validate_generated_leaf(&report_path, managed_overwrite)?;
    validate_generated_leaf(trait_path, managed_overwrite)?;
    std::fs::write(&report_path, report_json).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: report_path.to_string(),
            source: e,
        }
    })?;
    std::fs::write(trait_path, trait_text).map_err(|e| crate::environment::Error::Filesystem {
        path: trait_path.to_string(),
        source: e,
    })?;

    Ok(ImportWriteResult {
        trait_path: trait_path.to_string(),
        report_path: report_path.to_string(),
        copied_entries: copy_plan.entries.len(),
        managed_overwrite,
    })
}

/// Stage the root `trait.toml` package manifest for a canonical import
/// package (Group 95: `[package].status` is the sole status surface, so a
/// canonical package written by import must have one to be lifecycle-edited
/// afterward).
///
/// If `existing_package_root` already has a root package manifest, its bytes
/// are preserved verbatim (import must not clobber a team-edited status);
/// otherwise a fresh manifest is generated with `status = "draft"`.
/// No-op when `package_root` is not a canonical `.ctx/traits/<id>` package —
/// legacy flat packages have no separate package-manifest surface.
pub fn stage_package_manifest(
    package_root: &Utf8Path,
    stage_dir: &Utf8Path,
    existing_package_root: Option<&Utf8Path>,
    trait_id: &str,
    trait_name: &str,
) -> crate::Result<()> {
    if !crate::layout::is_canonical_package_root(package_root) {
        return Ok(());
    }
    let stage_manifest_path = crate::layout::package_manifest_path(stage_dir);
    let existing_manifest_path = existing_package_root.map(crate::layout::package_manifest_path);
    let manifest_text = match existing_manifest_path.filter(|path| path.is_file()) {
        Some(path) => read_regular_utf8_file(&path)?,
        None => crate::init::package_manifest_text(trait_id, trait_name)?,
    };
    std::fs::write(&stage_manifest_path, manifest_text).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: stage_manifest_path.to_string(),
            source: e,
        }
    })?;
    Ok(())
}

fn scan_directory(
    base: &Utf8Path,
    current: &Utf8Path,
    imported_dir: &Utf8Path,
    entries: &mut Vec<ImportCopyEntry>,
    warnings: &mut Vec<ImportCopyWarning>,
) -> crate::Result<()> {
    let reader = match fs::read_dir(current) {
        Ok(r) => r,
        Err(_) => {
            warnings.push(ImportCopyWarning::UnsafeSourceLayout {
                path: current.to_string(),
            });
            return Ok(());
        }
    };

    for entry in reader {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = match Utf8PathBuf::from_path_buf(entry.path()) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_symlink() {
            warnings.push(ImportCopyWarning::UnsafeSourceLayout {
                path: path.to_string(),
            });
            continue;
        }

        if file_type.is_dir() {
            scan_directory(base, &path, imported_dir, entries, warnings)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let relative = match path.strip_prefix(base) {
            Ok(rel) => rel.to_string(),
            Err(_) => continue,
        };

        // Reject unsafe relative paths without rejecting ordinary filenames
        // such as `notes..md`.
        if !is_safe_relative_path(&relative) {
            warnings.push(ImportCopyWarning::UnsafeSourceLayout {
                path: relative.clone(),
            });
            continue;
        }

        let destination = imported_dir.join(&relative).to_string();
        let conflict = Utf8Path::new(&destination).exists();

        if conflict {
            warnings.push(ImportCopyWarning::PathConflict {
                destination: destination.clone(),
            });
        }

        entries.push(ImportCopyEntry {
            source: path.to_string(),
            destination,
            conflict,
        });
    }

    Ok(())
}

/// Compute a deterministic digest over an import source.
///
/// Directories are digested as a sorted list of package-relative file paths and
/// per-file digests. Symlinks and special files are represented by sentinel
/// entries instead of being followed. Missing paths still produce a stable
/// digest so the import report can be emitted alongside copy-plan warnings.
pub fn digest_import_source(source: &Utf8Path) -> crate::Result<Digest> {
    let metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Digest::source(&format!("missing-import-source:{source}")));
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: source.to_string(),
                source: e,
            }
            .into());
        }
    };

    if metadata.file_type().is_symlink() {
        return Ok(Digest::source(&format!("symlink-import-source:{source}")));
    }

    if metadata.file_type().is_file() {
        let name = source.file_name().map_or(".", |name| name);
        return Ok(digest_entries(vec![(
            name.to_string(),
            digest_regular_import_file(source)?,
        )]));
    }

    if !metadata.file_type().is_dir() {
        return Ok(Digest::source(&format!(
            "unsupported-import-source:{source}"
        )));
    }

    let mut entries = Vec::new();
    collect_source_digests(source, source, &mut entries)?;
    Ok(digest_entries(entries))
}

fn collect_source_digests(
    base: &Utf8Path,
    current: &Utf8Path,
    entries: &mut Vec<(String, Digest)>,
) -> crate::Result<()> {
    let reader = fs::read_dir(current).map_err(|e| crate::environment::Error::Filesystem {
        path: current.to_string(),
        source: e,
    })?;

    for entry in reader {
        let entry = entry.map_err(|e| crate::environment::Error::Filesystem {
            path: current.to_string(),
            source: e,
        })?;

        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            crate::environment::Error::Filesystem {
                path: path.display().to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "import source path is not UTF-8",
                ),
            }
        })?;

        let relative = relative_import_path(base, &path)?;
        if !is_safe_relative_path(&relative) {
            entries.push((relative, Digest::source("unsafe-import-path")));
            continue;
        }

        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                entries.push((relative, Digest::source("missing-import-path")));
                continue;
            }
            Err(e) => {
                return Err(crate::environment::Error::Filesystem {
                    path: path.to_string(),
                    source: e,
                }
                .into());
            }
        };

        if metadata.file_type().is_symlink() {
            entries.push((relative, Digest::source("symlink-import-path")));
            continue;
        }

        if metadata.file_type().is_dir() {
            collect_source_digests(base, &path, entries)?;
            continue;
        }

        if metadata.file_type().is_file() {
            entries.push((relative, digest_regular_import_file(&path)?));
            continue;
        }

        entries.push((relative, Digest::source("special-import-path")));
    }

    Ok(())
}

fn copy_import_entry(entry: &ImportCopyEntry, allow_overwrite: bool) -> crate::Result<()> {
    let source = Utf8Path::new(&entry.source);
    let destination = Utf8Path::new(&entry.destination);
    match fs::symlink_metadata(source) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return filesystem_error(source, "import copy source must be a regular file");
            }
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: source.to_string(),
                source: e,
            }
            .into());
        }
    }

    let parent = destination
        .parent()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: destination.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "import copy destination has no parent directory",
            ),
        })?;
    validate_path_shape(destination)?;
    reject_existing_symlink_ancestors(parent)?;
    std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
        path: parent.to_string(),
        source: e,
    })?;
    reject_existing_symlink_ancestors(parent)?;
    validate_generated_leaf(destination, allow_overwrite)?;
    fs::copy(source, destination).map_err(|e| crate::environment::Error::Filesystem {
        path: destination.to_string(),
        source: e,
    })?;
    Ok(())
}

fn validate_import_trait_target(trait_path: &Utf8Path) -> crate::Result<bool> {
    match fs::symlink_metadata(trait_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return filesystem_error(
                    trait_path,
                    "import target must be absent or a regular generated file",
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: trait_path.to_string(),
                source: e,
            }
            .into());
        }
    }

    let Some(parent) = crate::layout::package_root_for_manifest(trait_path) else {
        return filesystem_error(trait_path, "import target has no parent directory");
    };
    let report_path = parent.join("import-report.json");
    match fs::symlink_metadata(&report_path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return filesystem_error(
                &report_path,
                "managed import report must be a regular file, not a symlink or special file",
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return filesystem_error(
                trait_path,
                "refusing to overwrite existing trait.toml without sibling import-report.json",
            );
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: report_path.to_string(),
                source: e,
            }
            .into());
        }
    }

    let existing_trait =
        fs::read_to_string(trait_path).map_err(|e| crate::environment::Error::Filesystem {
            path: trait_path.to_string(),
            source: e,
        })?;
    let existing_trait_digest = Digest::source(&existing_trait).as_str().to_string();
    let report_text =
        fs::read_to_string(&report_path).map_err(|e| crate::environment::Error::Filesystem {
            path: report_path.to_string(),
            source: e,
        })?;
    let report: ImportReport = serde_json::from_str(&report_text).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: report_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("existing canonical trait is unmanaged or has stale import evidence: import-report.json did not parse as generated import report: {e}"),
            ),
        }
    })?;
    let Some(marker) = report.managed_import.as_ref() else {
        return filesystem_error(
            &report_path,
            "existing canonical trait is unmanaged or has stale import evidence: missing managed-import marker",
        );
    };
    validate_managed_import_marker(
        &report_path,
        &report,
        marker,
        existing_trait_digest.as_str(),
    )?;
    Ok(true)
}

/// Validate whether an existing import package target may be overwritten.
///
/// Returns `true` only for an existing managed import package with current
/// generated evidence. Returns `false` when the target trait file is absent.
pub fn validate_managed_import_overwrite(trait_path: &Utf8Path) -> crate::Result<bool> {
    validate_path_shape(trait_path)?;
    let parent = trait_path
        .parent()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: trait_path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "import target has no parent directory",
            ),
        })?;
    reject_existing_symlink_ancestors(parent)?;
    validate_import_trait_target(trait_path)
}

fn validate_managed_import_marker(
    report_path: &Utf8Path,
    report: &ImportReport,
    marker: &ManagedImportArtifact,
    existing_trait_digest: &str,
) -> crate::Result<()> {
    if marker.schema_version != ManagedImportArtifact::SCHEMA_VERSION {
        return stale_import_evidence(report_path, "managed-import schema version is unsupported");
    }
    if marker.generated_by != ManagedImportArtifact::GENERATED_BY {
        return stale_import_evidence(
            report_path,
            "managed-import generator is not ctx-traits-import",
        );
    }
    if marker.trait_id.trim().is_empty() {
        return stale_import_evidence(report_path, "managed-import trait-id is empty");
    }
    if marker.raw_source_digest.trim().is_empty() {
        return stale_import_evidence(report_path, "managed-import raw-source-digest is empty");
    }
    if marker.trait_digest.trim().is_empty() {
        return stale_import_evidence(report_path, "managed-import trait digest is empty");
    }
    if marker.source_profile == ImportProfile::Unknown {
        return stale_import_evidence(report_path, "managed-import source-profile is unknown");
    }
    if marker.source_profile != report.source_profile {
        return stale_import_evidence(
            report_path,
            "managed-import source-profile does not match report",
        );
    }
    if marker.raw_source_digest != report.raw_source_digest {
        return stale_import_evidence(
            report_path,
            "managed-import raw-source-digest does not match report",
        );
    }
    if marker.trait_digest.as_str() != existing_trait_digest {
        return stale_import_evidence(
            report_path,
            "managed-import trait digest does not match existing trait.toml",
        );
    }
    Ok(())
}

fn stale_import_evidence<T>(path: &Utf8Path, detail: &str) -> crate::Result<T> {
    filesystem_error(
        path,
        &format!("existing canonical trait is unmanaged or has stale import evidence: {detail}"),
    )
}
