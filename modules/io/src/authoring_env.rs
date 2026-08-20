//! Whether this repository can build a trait from source, and what is missing
//! when it cannot.
//!
//! Building a trait runs the CDK through Node, so authoring needs three things
//! that RUNNING a trait does not: a Node runtime, an authoring manifest, and
//! the packages that manifest names installed somewhere Node will find them. A
//! generated `index.toml` is self-contained — none of this is on the run path.
//!
//! Every surface that needs those three asks here, so a user is told the same
//! thing by `init`, by `create`, and by a failed build, and is never told to
//! run a command that cannot fix their problem. That last part is why this
//! module exists: the build's own advice used to be "run `ctx traits init` to
//! install the authoring packages", which `init` has never done.

use camino::{Utf8Path, Utf8PathBuf};

/// One missing prerequisite, in the order a user would fix them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    /// No Node runtime on PATH. Nothing else can be checked past this.
    NodeRuntime,
    /// No package manager on PATH, so nothing can install the manifest.
    PackageManager,
    /// `.ctx/traits/package.json` is absent — the project was never
    /// initialised, or was initialised before the manifest existed.
    AuthoringManifest { path: Utf8PathBuf },
    /// The manifest exists but its packages are not installed.
    Install { root: Utf8PathBuf },
    /// The manifest declares a different supported range than this binary
    /// does. Not fatal on its own — the installed CDK may still fall inside
    /// both — but it means the project was initialised by another ctx.
    StaleRange { found: String, expected: String },
    /// The installed authoring packages do not match the digest recorded
    /// when they were installed. Fatal: a build reads these bytes, and the
    /// lock says they are not the bytes that were reviewed.
    ContentDrift { package: String },
    /// A CDK is installed, and it is outside the range this binary supports.
    /// Fatal: the canonical this binary writes and the one that CDK reads
    /// are different schemas, and the failure without this check is a type
    /// error from inside the CDK rather than a version problem.
    UnsupportedCdk {
        found: String,
        min: String,
        max: String,
    },
}

impl Missing {
    /// Whether this alone makes authoring impossible. A range that merely
    /// differs does not; an installed CDK outside the window does.
    pub fn blocks_authoring(&self) -> bool {
        !matches!(self, Self::StaleRange { .. })
    }

    /// What a user should read. Each names the one command that fixes it —
    /// and only when a command actually does.
    pub fn remedy(&self) -> String {
        match self {
            Self::NodeRuntime => "install Node (https://nodejs.org); building a trait from source \
                 runs the CDK through it. Running an already-built trait does not need Node."
                .to_string(),
            Self::PackageManager => {
                "install pnpm or npm; one of them has to install the authoring packages".to_string()
            }
            Self::AuthoringManifest { path } => {
                format!("run `ctx traits init` to write {path}")
            }
            Self::Install { root } => format!(
                "run `ctx traits init --install` to install the authoring packages into \
                 {root}/node_modules, or install them yourself from {root}"
            ),
            Self::StaleRange { found, expected } => format!(
                "the authoring manifest allows {found} but this ctx supports {expected}; run \
                 `ctx traits init --install` to move to {expected}"
            ),
            Self::ContentDrift { package } => format!(
                "{package} on disk does not match the digest recorded in config.lock; run \
                 `ctx traits init --install` to reinstall and re-record it"
            ),
            Self::UnsupportedCdk { found, min, max } => format!(
                "@ctx-traits/cdk {found} is installed, but this ctx supports >={min} <{max}; run \
                 `ctx traits init --install` to install one it can build with"
            ),
        }
    }
}

/// What this repository is missing for authoring, in fix order. Empty means
/// a trait can be built from source here.
pub fn missing_for_authoring(repo_root: &Utf8Path, expected_range: &str) -> Vec<Missing> {
    let mut missing = Vec::new();

    if which("node").is_none() {
        // Everything below depends on Node, so report it alone: a list that
        // also demanded a package manager would make the user fix two things
        // to learn whether the second was ever a problem.
        missing.push(Missing::NodeRuntime);
        return missing;
    }

    let manifest = crate::layout::authoring_manifest_path(repo_root);
    if !manifest.is_file() {
        missing.push(Missing::AuthoringManifest {
            path: manifest.clone(),
        });
    } else if let Some(found) = declared_range(&manifest)
        && found != expected_range
    {
        missing.push(Missing::StaleRange {
            found,
            expected: expected_range.to_string(),
        });
    }

    // An installed CDK outside the window is reported ahead of a missing
    // install: it IS installed, and telling someone to install what they
    // already have explains nothing.
    if let Some(found) = installed_cdk_version(repo_root) {
        let (min, max) = crate::init::authoring_cdk_range();
        if !version_within(&found, &min, &max) {
            missing.push(Missing::UnsupportedCdk { found, min, max });
        }
        missing.extend(drifted_authoring_packages(repo_root));
    }

    if !authoring_packages_installed(repo_root) {
        if package_manager(repo_root).is_none() {
            missing.push(Missing::PackageManager);
        }
        missing.push(Missing::Install {
            root: crate::layout::authoring_install_root(repo_root),
        });
    }

    missing
}

/// Whether `@ctx-traits/cdk` resolves from somewhere Node will find it.
///
/// Checks the two roots ctx installs into and the repository root, which is
/// where a project with its own JavaScript setup would have it. It does not
/// shell out to Node: this runs before every authoring command, and a process
/// spawn per invocation to learn a directory exists is not worth it.
pub fn authoring_packages_installed(repo_root: &Utf8Path) -> bool {
    [
        crate::layout::authoring_install_root(repo_root),
        repo_root.join(".ctx"),
        repo_root.to_path_buf(),
    ]
    .iter()
    .any(|root| {
        root.join("node_modules")
            .join("@ctx-traits")
            .join("cdk")
            .exists()
    })
}

/// The package manager an install should use here.
///
/// A lockfile already in the authoring root wins: whatever produced it is what
/// the project has committed to, and switching managers underneath a lockfile
/// is how a working tree grows two of them. Otherwise pnpm, then npm.
pub fn package_manager(repo_root: &Utf8Path) -> Option<PackageManager> {
    let root = crate::layout::authoring_install_root(repo_root);
    if root.join("pnpm-lock.yaml").is_file() && which("pnpm").is_some() {
        return Some(PackageManager::Pnpm);
    }
    if root.join("package-lock.json").is_file() && which("npm").is_some() {
        return Some(PackageManager::Npm);
    }
    if which("pnpm").is_some() {
        return Some(PackageManager::Pnpm);
    }
    which("npm").map(|_| PackageManager::Npm)
}

/// The package managers an authoring install can run through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Pnpm,
    Npm,
}

impl PackageManager {
    pub fn binary(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm",
            Self::Npm => "npm",
        }
    }

    /// The lockfile this manager writes, which is also what identifies it on
    /// a later run.
    pub fn lockfile(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm-lock.yaml",
            Self::Npm => "package-lock.json",
        }
    }

    fn install_args(self) -> &'static [&'static str] {
        match self {
            // `--ignore-workspace` because the authoring root is not a member
            // of whatever workspace may sit above it; without it pnpm resolves
            // against the outer workspace and installs nothing here.
            Self::Pnpm => &["install", "--ignore-workspace"],
            Self::Npm => &["install"],
        }
    }
}

/// Install the authoring packages, returning the manager that ran.
///
/// Runs in the authoring root so `node_modules` and the lockfile land beside
/// the manifest. Inherits stdio: an install is slow enough that silence reads
/// as a hang, and its own progress output is better than anything relayed.
pub fn install_authoring_packages(repo_root: &Utf8Path) -> crate::Result<PackageManager> {
    let manager = package_manager(repo_root).ok_or_else(|| crate::environment::Error::Process {
        command: Some("pnpm|npm install".to_string()),
        path: None,
        exit_status: None,
        timed_out: false,
        message: Missing::PackageManager.remedy(),
    })?;
    let root = crate::layout::authoring_install_root(repo_root);
    let status = std::process::Command::new(manager.binary())
        .args(manager.install_args())
        .current_dir(root.as_std_path())
        .status()
        .map_err(|source| crate::environment::Error::Process {
            command: Some(format!("{} install", manager.binary())),
            path: Some(root.to_string()),
            exit_status: None,
            timed_out: false,
            message: source.to_string(),
        })?;
    if !status.success() {
        return Err(crate::environment::Error::Process {
            command: Some(format!("{} install", manager.binary())),
            path: Some(root.to_string()),
            exit_status: status.code(),
            timed_out: false,
            message: "the authoring packages are not installed".to_string(),
        }
        .into());
    }
    Ok(manager)
}

/// Which recorded authoring packages no longer match what is on disk.
///
/// Best-effort by design: no lock, no entry, or an unreadable tree yields
/// nothing rather than an error. This runs before ordinary commands, and a
/// project that has never installed is a state to report, not a failure.
fn drifted_authoring_packages(repo_root: &Utf8Path) -> Vec<Missing> {
    let Ok(Some(lock)) = crate::project_lock::read_project_lock(repo_root) else {
        return Vec::new();
    };
    let Some(authoring) = lock.authoring else {
        return Vec::new();
    };
    authoring
        .packages
        .iter()
        .filter(|entry| !entry.tree_digest.is_empty())
        .filter_map(|entry| {
            let root = installed_package_root(repo_root, &entry.package)?;
            let actual = crate::registry::compute_tree_digest(&root).ok()?;
            (actual != entry.tree_digest).then(|| Missing::ContentDrift {
                package: entry.package.clone(),
            })
        })
        .collect()
}

/// The packages an authoring install provides, in the order recorded.
pub const AUTHORING_PACKAGES: &[&str] = &["@ctx-traits/cdk", "@ctx-traits/config"];

/// Where an installed authoring package's real files are.
///
/// Canonicalized, because pnpm links `node_modules/@ctx-traits/cdk` into
/// `node_modules/.pnpm/...` and the digest walk rejects symlinks — correctly,
/// since a tree that can point elsewhere is not evidence of its own content.
fn installed_package_root(repo_root: &Utf8Path, package: &str) -> Option<Utf8PathBuf> {
    [
        crate::layout::authoring_install_root(repo_root),
        repo_root.join(".ctx"),
        repo_root.to_path_buf(),
    ]
    .iter()
    .find_map(|root| {
        let mut path = root.join("node_modules");
        for segment in package.split('/') {
            path = path.join(segment);
        }
        path.join("package.json")
            .is_file()
            .then(|| path.canonicalize_utf8().ok())
            .flatten()
    })
}

/// Digest what is actually installed, and record it.
///
/// The package manager's lockfile is not evidence here. It records what a
/// registry said and is rewritten on that tool's own schedule; our lock
/// digests the bytes a build will read. Same rule, and the same digest
/// scheme, as the vendored-tree evidence in `PackageLockEntry::tree_digest`.
pub fn authoring_lock_entry(
    repo_root: &Utf8Path,
    manager: PackageManager,
    range: &str,
) -> crate::Result<ctx_traits_core::project_lock::AuthoringLock> {
    let mut packages = Vec::new();
    for name in AUTHORING_PACKAGES {
        let Some(root) = installed_package_root(repo_root, name) else {
            continue;
        };
        let version = std::fs::read_to_string(root.join("package.json").as_std_path())
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| value.get("version")?.as_str().map(str::to_string))
            .unwrap_or_default();
        packages.push(ctx_traits_core::project_lock::AuthoringPackageLock {
            package: (*name).to_string(),
            version,
            tree_digest: crate::registry::compute_tree_digest(&root)?,
        });
    }
    Ok(ctx_traits_core::project_lock::AuthoringLock {
        range: range.to_string(),
        manager: manager.binary().to_string(),
        packages,
    })
}

/// The version of `@ctx-traits/cdk` actually installed, from wherever Node
/// would resolve it.
pub fn installed_cdk_version(repo_root: &Utf8Path) -> Option<String> {
    [
        crate::layout::authoring_install_root(repo_root),
        repo_root.join(".ctx"),
        repo_root.to_path_buf(),
    ]
    .iter()
    .find_map(|root| {
        let manifest = root
            .join("node_modules")
            .join("@ctx-traits")
            .join("cdk")
            .join("package.json");
        let text = std::fs::read_to_string(manifest.as_std_path()).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        value.get("version")?.as_str().map(str::to_string)
    })
}

/// Whether `found` falls in `[min, max)`. Compares numerically rather than
/// lexically: "0.10.0" is above "0.9.0" and a string compare says otherwise.
fn version_within(found: &str, min: &str, max: &str) -> bool {
    let parse = |text: &str| -> Vec<u64> {
        text.split(['.', '-', '+'])
            .filter_map(|part| part.parse::<u64>().ok())
            .collect()
    };
    let (found, min, max) = (parse(found), parse(min), parse(max));
    found >= min && found < max
}

/// The range an authoring manifest declares for `@ctx-traits/cdk`.
fn declared_range(manifest: &Utf8Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest.as_std_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("dependencies")?
        .get("@ctx-traits/cdk")?
        .as_str()
        .map(str::to_string)
}

/// Whether `binary` is on PATH.
fn which(binary: &str) -> Option<Utf8PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(binary);
        candidate
            .is_file()
            .then(|| Utf8PathBuf::from_path_buf(candidate).ok())
            .flatten()
    })
}
