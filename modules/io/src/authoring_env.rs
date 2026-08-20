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
    /// The manifest pins a different version than this binary scaffolds.
    /// Not fatal: the install may still resolve and build. Reported so an
    /// upgrade is visible rather than surfacing later as a schema error.
    StalePin { found: String, expected: String },
}

impl Missing {
    /// Whether this alone makes authoring impossible. A stale pin does not.
    pub fn blocks_authoring(&self) -> bool {
        !matches!(self, Self::StalePin { .. })
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
            Self::StalePin { found, expected } => format!(
                "the authoring manifest pins {found} but this ctx scaffolds {expected}; run \
                 `ctx traits init --install` to move to {expected}"
            ),
        }
    }
}

/// What this repository is missing for authoring, in fix order. Empty means
/// a trait can be built from source here.
pub fn missing_for_authoring(repo_root: &Utf8Path, expected_pin: &str) -> Vec<Missing> {
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
    } else if let Some(found) = pinned_version(&manifest)
        && found != expected_pin
    {
        missing.push(Missing::StalePin {
            found,
            expected: expected_pin.to_string(),
        });
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

/// The version an authoring manifest pins `@ctx-traits/cdk` to.
fn pinned_version(manifest: &Utf8Path) -> Option<String> {
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
