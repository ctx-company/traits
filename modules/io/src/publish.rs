//! Safe local staging and npm publication (P440).

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("publish requires exactly one trait package, found {0}")]
    PackageCount(usize),
    #[error("trait package is not ready: [package].status is {0}")]
    NotReady(String),
    #[error("npm version {package}@{version} is already published")]
    VersionExists { package: String, version: String },
    #[error("npm {operation} failed: {message}")]
    Npm { operation: String, message: String },
    #[error("unsafe publish input {path}: {message}")]
    Unsafe { path: String, message: String },
    #[error("publish metadata error: {0}")]
    Metadata(String),
    #[error(
        "direct trait packages require an existing npm wrapper package with a publish-ready runtime export"
    )]
    DirectPackageRequiresWrapper,
}

/// Directory names never published to the tarball, at any depth, absent a
/// declared `[publish] exclude` override. The one exclude authority:
/// `copy_safe` below and `distribution::collect_package_files` both call
/// [`is_pack_excluded`] against the same resolved `excludes` list instead of
/// carrying their own hand-rolled copy, so the staged tarball and the
/// pre-skip manifest that verifies it can never independently drift.
pub const PACK_DEFAULT_EXCLUDES: &[&str] = &[".git", "node_modules", "target", ".turbo"];

/// `true` when `name` (a bare directory name, not a path) matches one of
/// `excludes` — the resolved `[publish] exclude` override, or
/// [`PACK_DEFAULT_EXCLUDES`] when unconfigured (see
/// [`crate::harness_config::resolve_pack_excludes`]).
pub(crate) fn is_pack_excluded(name: &str, excludes: &[String]) -> bool {
    excludes.iter().any(|excluded| excluded == name)
}

#[derive(Debug, Clone)]
pub struct PublishInput {
    pub root: Utf8PathBuf,
    pub package: String,
    pub version: String,
    pub canonical_digests: BTreeMap<String, String>,
    pub required_paths: Vec<String>,
    /// Directories the default pack exclude set dropped from the walk (P489):
    /// carried straight through into [`ctx_traits_core::distribution::PublishPlan::skipped`]
    /// so the report names what was left out.
    pub excluded: Vec<ctx_traits_core::distribution::SkippedPath>,
    pub force_identity: bool,
    pub provenance: bool,
    pub json: bool,
}

pub fn publish(
    input: &PublishInput,
    dry_run: bool,
) -> crate::Result<ctx_traits_core::distribution::PublishPlan> {
    if input.force_identity && !input.root.join("package.json").is_file() {
        return Err(Error::DirectPackageRequiresWrapper.into());
    }
    check_version(&input.package, &input.version)?;
    let staging = staging_path();
    let result: crate::Result<ctx_traits_core::distribution::PublishPlan> = (|| {
        let excludes = crate::harness_config::resolve_pack_excludes(&input.root);
        copy_safe(&input.root, &staging, &excludes)?;
        let package_json = staging.join("package.json");
        let mut metadata = read_metadata(
            &package_json,
            &input.package,
            &input.version,
            input.force_identity,
        )?;
        metadata["ctx"] = json!({"digests": input.canonical_digests});
        metadata
            .as_object_mut()
            .expect("metadata object")
            .remove("scripts");
        rewrite_workspace_dependencies(&mut metadata, &input.root)?;
        std::fs::write(
            &package_json,
            serde_json::to_vec_pretty(&metadata).map_err(|e| Error::Metadata(e.to_string()))?,
        )
        .map_err(|source| filesystem(&package_json, source))?;
        let claim = crate::registry::load_publisher_claim(&staging)?.ok_or_else(|| {
            Error::Metadata("generated ctx.digests claim was not readable".to_string())
        })?;
        ctx_traits_core::distribution::verify_publisher_claim(
            Some(&claim),
            &input.canonical_digests,
        )
        .map_err(ctx_traits_core::Error::from)?;
        let packed = pack_check(&staging)?;
        let files = file_report(&staging, &packed)?;
        let packed_set: std::collections::BTreeSet<_> = packed.iter().cloned().collect();
        let missing: Vec<_> = input
            .required_paths
            .iter()
            .filter(|path| !packed_set.contains(*path))
            .cloned()
            .collect();
        let missing_claims: Vec<_> = input
            .canonical_digests
            .keys()
            .filter(|path| !packed_set.contains(*path))
            .cloned()
            .collect();
        if !missing.is_empty() || !missing_claims.is_empty() {
            return Err(Error::Npm {
                operation: "pack --dry-run".to_string(),
                message: format!("npm pack omitted required paths: {missing:?}; claimed paths missing: {missing_claims:?}"),
            }
            .into());
        }
        let plan = ctx_traits_core::distribution::PublishPlan {
            package: input.package.clone(),
            version: input.version.clone(),
            provenance: input.provenance,
            files,
            digests: input.canonical_digests.clone(),
            skipped: input.excluded.clone(),
        };
        if !dry_run {
            let mut argv = vec![
                "npm".to_string(),
                "publish".to_string(),
                staging.to_string(),
            ];
            if input.provenance {
                argv.push("--provenance".to_string());
            }
            let status = crate::command::run_interactive(&argv, &input.root, input.json).map_err(
                |source| Error::Npm {
                    operation: "publish".to_string(),
                    message: source.to_string(),
                },
            )?;
            if !status.success() {
                return Err(Error::Npm {
                    operation: "publish".to_string(),
                    message: status.to_string(),
                }
                .into());
            }
        }
        Ok(plan)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn check_version(package: &str, version: &str) -> crate::Result<()> {
    let argv = vec![
        "npm".to_string(),
        "view".to_string(),
        format!("{package}@{version}"),
        "version".to_string(),
        "--json".to_string(),
    ];
    let output = crate::command::run(crate::command::RunRequest {
        argv: &argv,
        cwd: None,
        exec_dir: None,
        success_exit_code: &[0],
        timeout_ms: Some(120_000),
        idle_timeout_ms: None,
        capture_limit: 16_384,
        tick_observer: None,
    })?;
    if output.success {
        return Err(Error::VersionExists {
            package: package.to_string(),
            version: version.to_string(),
        }
        .into());
    }
    if output.stderr.contains("E404")
        || output.stdout.contains("E404")
        || output.stderr.contains("404 Not Found")
        || output.stdout.contains("404 Not Found")
    {
        return Ok(());
    }
    Err(Error::Npm {
        operation: "version preflight".to_string(),
        message: output.stderr,
    }
    .into())
}

fn staging_path() -> Utf8PathBuf {
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .unwrap_or_else(|_| Utf8PathBuf::from("/tmp"))
        .join(format!("ctx-traits-publish-{suffix}"))
}

/// No-symlink, no-special-file, exclude-aware recursive copy of `source`
/// into `dest`. The one safe local-tree copy primitive in this crate: npm
/// publication staging (this module) and P535 local `path:` package staging
/// ([`crate::distribution::stage_local_package`]) both call this rather than
/// each walking the source tree themselves.
pub(crate) fn copy_safe(
    source: &Utf8Path,
    dest: &Utf8Path,
    excludes: &[String],
) -> crate::Result<()> {
    let metadata = std::fs::symlink_metadata(source).map_err(|e| filesystem(source, e))?;
    if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
        return Err(Error::Unsafe {
            path: source.to_string(),
            message: "symlinks and special files are not publishable".to_string(),
        }
        .into());
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(dest).map_err(|e| filesystem(dest, e))?;
        let mut entries = std::fs::read_dir(source)
            .map_err(|e| filesystem(source, e))?
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| filesystem(source, e))?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_pack_excluded(&name, excludes) {
                continue;
            }
            copy_safe(&source.join(&name), &dest.join(&name), excludes)?;
        }
    } else {
        std::fs::copy(source, dest).map_err(|e| filesystem(dest, e))?;
    }
    Ok(())
}

fn read_metadata(
    path: &Utf8Path,
    package: &str,
    version: &str,
    force_identity: bool,
) -> crate::Result<Value> {
    let value = if path.is_file() {
        serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| filesystem(path, e))?)
            .map_err(|e| Error::Metadata(e.to_string()))?
    } else {
        json!({})
    };
    let mut value = value;
    let object = value
        .as_object_mut()
        .ok_or_else(|| Error::Metadata("package.json must be an object".to_string()))?;
    if force_identity {
        if !path.is_file() {
            return Err(Error::DirectPackageRequiresWrapper.into());
        }
        object.insert("name".to_string(), Value::String(package.to_string()));
        object.insert("version".to_string(), Value::String(version.to_string()));
        // No runtime-entry requirement. A trait package's entry point is its
        // `trait.toml` and `generated/` tree, which ctx reads directly — a
        // consumer never imports it as JavaScript. The old check demanded
        // `exports`/`main`/`module` because the first package to travel this
        // path was dual-use TypeScript (`@ctx-traits/agents`); applying it to
        // every trait package forced authors to declare an entry point that
        // resolves to nothing, so the manifest asserted something untrue in
        // order to satisfy a check. A package that DOES ship JavaScript still
        // declares its own entry, and that declaration is preserved above.
    } else {
        object
            .entry("name")
            .or_insert_with(|| Value::String(package.to_string()));
        object
            .entry("version")
            .or_insert_with(|| Value::String(version.to_string()));
    }
    Ok(value)
}

fn rewrite_workspace_dependencies(
    metadata: &mut Value,
    package_root: &Utf8Path,
) -> crate::Result<()> {
    let Some(object) = metadata.as_object_mut() else {
        return Ok(());
    };
    for section in ["dependencies", "optionalDependencies", "peerDependencies"] {
        let Some(dependencies) = object.get_mut(section).and_then(Value::as_object_mut) else {
            continue;
        };
        for (name, version) in dependencies.iter_mut() {
            if version.as_str() != Some("workspace:*") {
                continue;
            }
            let resolved = find_workspace_version(package_root, name).ok_or_else(|| {
                Error::Metadata(format!("cannot resolve workspace dependency {name}"))
            })?;
            *version = Value::String(resolved);
        }
    }
    Ok(())
}

fn find_workspace_version(package_root: &Utf8Path, name: &str) -> Option<String> {
    let mut root = package_root;
    loop {
        for directory in ["packages", "modules"] {
            let candidate = root.join(directory);
            if let Ok(entries) = std::fs::read_dir(candidate) {
                for entry in entries.flatten() {
                    let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                        continue;
                    };
                    let path = path.join("package.json");
                    let Ok(text) = std::fs::read_to_string(path) else {
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if value.get("name").and_then(Value::as_str) == Some(name) {
                        return value
                            .get("version")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                    }
                }
            }
        }
        root = root.parent()?;
    }
}

fn file_report(
    root: &Utf8Path,
    paths: &[String],
) -> crate::Result<Vec<ctx_traits_core::distribution::PublishFile>> {
    let mut files = Vec::new();
    for relative in paths {
        let path = root.join(relative);
        let bytes = std::fs::read(&path).map_err(|e| filesystem(&path, e))?;
        let digest = format!(
            "sha256:{}",
            Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        files.push(ctx_traits_core::distribution::PublishFile {
            path: relative.clone(),
            digest,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn pack_check(staging: &Utf8Path) -> crate::Result<Vec<String>> {
    let argv = vec![
        "npm".to_string(),
        "pack".to_string(),
        "--dry-run".to_string(),
        "--json".to_string(),
    ];
    let output = crate::command::run(crate::command::RunRequest {
        argv: &argv,
        cwd: None,
        exec_dir: Some(staging),
        success_exit_code: &[0],
        timeout_ms: Some(120_000),
        idle_timeout_ms: None,
        capture_limit: 1_000_000,
        tick_observer: None,
    })?;
    if output.success {
        // Preserve the shared refusal message rather than re-authoring it
        // with a locally hand-copied cap.
        output
            .refuse_if_truncated("npm pack --dry-run --json")
            .map_err(|error| Error::Npm {
                operation: "pack --dry-run".to_string(),
                message: error.to_string(),
            })?;
        let value: Value = serde_json::from_str(&output.stdout).map_err(|source| Error::Npm {
            operation: "pack --dry-run".to_string(),
            message: format!("invalid npm JSON output: {source}"),
        })?;
        let files = value
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item.get("files"))
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Npm {
                operation: "pack --dry-run".to_string(),
                message: "npm output has no file list".to_string(),
            })?;
        Ok(files
            .iter()
            .filter_map(|file| file.get("path").and_then(Value::as_str))
            .map(|path| path.strip_prefix("package/").unwrap_or(path).to_string())
            .collect())
    } else {
        Err(Error::Npm {
            operation: "pack --dry-run".to_string(),
            message: output.stderr,
        }
        .into())
    }
}

fn filesystem(path: &Utf8Path, source: std::io::Error) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source,
    }
    .into()
}
