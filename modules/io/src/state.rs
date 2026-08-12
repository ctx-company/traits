//! Global per-repository runtime state (P426).
//!
//! Default run ledgers, debug traces, and cache families live under
//! `~/.config/ctx/{runs,debug,cache}/<repo-key>/` rather than repo-local
//! `.ctx/`, so machine-local runtime state survives worktree/branch churn and
//! never pollutes a repository's working tree. `<repo-key>` is derived from
//! the canonical (symlink-resolved) repository path as
//! `<dirname-slug>-<first-8-sha256-hex>`, so two aliases of one checkout
//! collapse to the same key while two repositories that merely share a
//! basename never collide.
//!
//! `.ctx/traits` (trait packages), `.ctx/traits/profiles` config, and
//! `.ctx/worktrees` are project content or deliberately repo-local and are
//! never touched by anything in this module — see [`crate::layout`]'s module
//! doc for that boundary.

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config-home resolution (shared with `crate::trust` and
// `crate::harness_config`'s global runtime config so there is exactly one
// XDG/HOME resolution rule in the crate).
// ---------------------------------------------------------------------------

/// Base config-home directory: `$XDG_CONFIG_HOME` if set and non-empty,
/// otherwise `$HOME/.config`. Errors if neither is available.
///
/// Resolved to its PHYSICAL path when it exists: dotfiles setups routinely
/// make `~/.config` itself a symlink (e.g. `-> ~/dev/dotfiles/config`), and
/// the run-session / export / lock-evidence writers refuse any symlinked
/// ancestor as an integrity guard. Canonicalizing once here — exactly like
/// [`canonical_repo_root`] does for repo keying — keeps every derived state
/// path symlink-free without loosening those guards. A not-yet-existing
/// config home passes through verbatim (nothing to resolve; the first
/// `create_dir_all` materializes a real directory).
pub fn config_home_dir() -> crate::Result<Utf8PathBuf> {
    let nominal = if let Some(value) =
        std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty())
    {
        utf8_path(value)?
    } else {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| crate::environment::Error::Filesystem {
                path: "~/.config".to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "HOME is not set and XDG_CONFIG_HOME was not provided",
                ),
            })?;
        utf8_path(home)?.join(".config")
    };
    match std::fs::canonicalize(nominal.as_std_path()) {
        Ok(canonical) => Utf8PathBuf::from_path_buf(canonical).map_err(|path| {
            crate::environment::Error::Filesystem {
                path: path.display().to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "canonical config-home path is not valid UTF-8",
                ),
            }
            .into()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(nominal),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: nominal.to_string(),
            source,
        }
        .into()),
    }
}

/// Global `ctx` config-home root (`$XDG_CONFIG_HOME/ctx` or
/// `$HOME/.config/ctx`). Houses [`traits/runtime.toml`](crate::layout::GLOBAL_RUNTIME_CONFIG),
/// [`crate::trust`]'s store, `checkouts.toml`, and the `runs`/`debug`/`cache`
/// per-repository roots this module resolves.
pub fn global_ctx_root() -> crate::Result<Utf8PathBuf> {
    Ok(config_home_dir()?.join("ctx"))
}

/// The user's home directory (`$HOME`), used for host-placement global
/// scope: several host tools (Cursor, Cline, Kiro, ...) keep their
/// user-level configuration directly under the home directory rather than
/// under the XDG config home that [`config_home_dir`] resolves.
pub fn home_dir() -> crate::Result<Utf8PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: "~".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"),
        })?;
    utf8_path(home)
}

/// Global host-placement manifest path
/// (`${XDG_CONFIG_HOME:-$HOME/.config}/ctx/host-placements.toml`), the
/// machine-wide sibling of the project-local
/// [`crate::layout::project_host_placements_manifest_path`].
pub fn global_host_placements_manifest_path() -> crate::Result<Utf8PathBuf> {
    Ok(global_ctx_root()?.join("host-placements.toml"))
}

/// Root directory for the machine-wide host-placement audit journal
/// (`${XDG_CONFIG_HOME:-$HOME/.config}/ctx/audit`), holding one
/// `YYYY-MM.jsonl` file per month.
pub fn global_audit_root() -> crate::Result<Utf8PathBuf> {
    Ok(global_ctx_root()?.join("audit"))
}

fn utf8_path(value: std::ffi::OsString) -> crate::Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(std::path::PathBuf::from(value)).map_err(|path| {
        crate::environment::Error::Filesystem {
            path: path.display().to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path is not valid UTF-8",
            ),
        }
        .into()
    })
}

// ---------------------------------------------------------------------------
// Repository identity
// ---------------------------------------------------------------------------

/// Canonicalize a repository root (resolve symlinks) so every alias of the
/// same checkout derives the same [`repo_key`].
pub fn canonical_repo_root(repo_root: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let canonical = std::fs::canonicalize(repo_root.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: repo_root.to_string(),
            source,
        }
    })?;
    Utf8PathBuf::from_path_buf(canonical).map_err(|path| {
        crate::environment::Error::Filesystem {
            path: path.display().to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical repository path is not valid UTF-8",
            ),
        }
        .into()
    })
}

/// Deterministic repo key: `<dirname-slug>-<first-8-sha256-hex>` of the
/// canonical repository path. Hashing the full path (not just the basename)
/// means two repositories that merely share a directory name never collide;
/// slugging the basename keeps the key human-recognizable.
pub fn repo_key(canonical_repo_root: &Utf8Path) -> String {
    let name = canonical_repo_root.file_name().unwrap_or("repo");
    let slug = crate::debug_trace::slug(name);
    let digest = ctx_traits_core::digest::Digest::source(canonical_repo_root.as_str());
    let hex = digest.as_str().trim_start_matches("sha256:");
    let short = &hex[..hex.len().min(8)];
    format!("{slug}-{short}")
}

fn current_dir_utf8() -> crate::Result<Utf8PathBuf> {
    let cwd = std::env::current_dir().map_err(|source| crate::environment::Error::Filesystem {
        path: ".".to_string(),
        source,
    })?;
    Utf8PathBuf::from_path_buf(cwd).map_err(|path| {
        crate::environment::Error::Filesystem {
            path: path.display().to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "current directory is not valid UTF-8",
            ),
        }
        .into()
    })
}

/// Identity of the invocation working directory: a genuine Git repository
/// root, or the plain (ad-hoc) working directory when the cwd is not inside
/// a Git worktree at all. Distinguishes that expected "no repository" case
/// from a genuine Git operational failure (unsafe/dubious ownership,
/// corrupted `.git`, malformed config), which is never folded into either
/// variant and instead surfaces as `Err` from
/// [`discover_invocation_root`] — see
/// [`crate::repository::discover_repo_root_at`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationRoot {
    Repo(Utf8PathBuf),
    Adhoc(Utf8PathBuf),
}

impl InvocationRoot {
    pub fn path(&self) -> &Utf8Path {
        match self {
            Self::Repo(path) | Self::Adhoc(path) => path,
        }
    }
}

/// Discover the invocation's repository identity (P426/P439): the Git
/// worktree root containing the current directory, or that same current
/// directory tagged `Adhoc` when it is genuinely outside any Git repository.
/// A genuine Git discovery error (as opposed to "not a repository") is
/// returned as `Err`, never silently downgraded to `Adhoc`.
pub fn discover_invocation_root() -> crate::Result<InvocationRoot> {
    let cwd = current_dir_utf8()?;
    match crate::repository::discover_repo_root_at(&cwd)? {
        Some(root) => Ok(InvocationRoot::Repo(root)),
        None => Ok(InvocationRoot::Adhoc(cwd)),
    }
}

/// Repository root used for state keying: the invocation Git worktree root
/// if one exists, otherwise the current working directory. Never hard-fails
/// merely for running outside a Git repo — only a genuine Git operational
/// error (not "no repository here") propagates as `Err`.
pub fn state_repo_root() -> crate::Result<Utf8PathBuf> {
    Ok(discover_invocation_root()?.path().to_path_buf())
}

/// The project root usable for building repo-relative trait/manifest paths,
/// expressed relative to the current working directory.
///
/// Project-tier (`.ctx/traits`) resolution has never required a Git
/// repository — a plain directory with a `.ctx/traits/config.toml` manifest is a
/// valid project on its own — so an ad-hoc (non-repository) invocation
/// resolves to `.` here, exactly the literal cwd it has always used.
/// Inside a Git repository, this resolves to `.` when the invocation cwd is
/// already the repository root — the overwhelmingly common case, and the
/// one whose exact relative-path output (`./.ctx/traits/...`) prior
/// releases and byte-stability goldens pin — or the correct
/// `../..`-style ascent when invoked from a subdirectory of the repository
/// (P439: without this, project-tier resolution silently used the literal
/// cwd instead of the actual repository root, so running from a
/// subdirectory could miss a nearer project trait entirely, or let a
/// farther tier incorrectly win).
pub fn repo_root_for_relative_paths() -> crate::Result<Utf8PathBuf> {
    repo_root_for_relative_paths_from(&discover_invocation_root()?)
}

/// Like [`repo_root_for_relative_paths`], but reuses an already-discovered
/// [`InvocationRoot`] instead of re-running Git discovery — for callers
/// (such as [`crate::inventory`]) that discover the invocation once and
/// need this derived value alongside it.
pub fn repo_root_for_relative_paths_from(
    invocation: &InvocationRoot,
) -> crate::Result<Utf8PathBuf> {
    match invocation {
        InvocationRoot::Adhoc(_) => Ok(Utf8PathBuf::from(".")),
        InvocationRoot::Repo(repo_root) => {
            let canonical_cwd = canonical_repo_root(&current_dir_utf8()?)?;
            let canonical_repo = canonical_repo_root(repo_root)?;
            if canonical_cwd == canonical_repo {
                return Ok(Utf8PathBuf::from("."));
            }
            let suffix = canonical_cwd.strip_prefix(&canonical_repo).map_err(|_| {
                crate::environment::Error::Filesystem {
                    path: repo_root.to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "current directory is not inside the discovered repository root",
                    ),
                }
            })?;
            let mut relative = Utf8PathBuf::new();
            for _ in suffix.components() {
                relative.push("..");
            }
            Ok(relative)
        }
    }
}

/// Repo key for the current invocation: the ordinary `<slug>-<hash>` key
/// inside a Git repository, or an `adhoc-`-prefixed key of the same shape
/// derived from the canonical cwd when the invocation is outside any Git
/// repository (P439). Two ad-hoc runs from the same directory therefore
/// collapse to the same key, exactly like two invocations of one repo
/// checkout do.
pub fn current_repo_key() -> crate::Result<String> {
    match discover_invocation_root()? {
        InvocationRoot::Repo(root) => Ok(repo_key(&canonical_repo_root(&root)?)),
        InvocationRoot::Adhoc(cwd) => {
            Ok(format!("adhoc-{}", repo_key(&canonical_repo_root(&cwd)?)))
        }
    }
}

// ---------------------------------------------------------------------------
// Global and legacy family roots
// ---------------------------------------------------------------------------

pub fn global_runs_root(repo_key: &str) -> crate::Result<Utf8PathBuf> {
    Ok(global_ctx_root()?.join("runs").join(repo_key))
}

pub fn global_debug_root(repo_key: &str) -> crate::Result<Utf8PathBuf> {
    Ok(global_ctx_root()?.join("debug").join(repo_key))
}

pub fn global_cache_root(repo_key: &str) -> crate::Result<Utf8PathBuf> {
    Ok(global_ctx_root()?.join("cache").join(repo_key))
}

/// Global runs root for the current invocation's repository.
pub fn current_global_runs_root() -> crate::Result<Utf8PathBuf> {
    global_runs_root(&current_repo_key()?)
}

/// Global debug root for the current invocation's repository.
pub fn current_global_debug_root() -> crate::Result<Utf8PathBuf> {
    global_debug_root(&current_repo_key()?)
}

/// Global cache root for the current invocation's repository.
pub fn current_global_cache_root() -> crate::Result<Utf8PathBuf> {
    global_cache_root(&current_repo_key()?)
}

/// Global per-repository context-ledger root (P498): a sibling of `runs`,
/// `debug`, and `cache` under `~/.config/ctx/context/<repo-key>/`, never
/// nested under `cache/`. It is a brand-new state family with no
/// pre-P498 repo-local predecessor, so it deliberately has:
/// - no entry in [`StateFamily`] or [`plan_migration`]/`apply_migration`
///   (nothing to migrate — it never had a legacy repo-local location);
/// - no pruning path anywhere (`cache prune`, `cache rebuild`, and every
///   other prune surface only ever walk `cache/`) — a context-ledger entry
///   is evidence of what a host session was supplied and is retained for
///   the life of that host session's ledger file, not pruned by mtime, TTL,
///   or artifact staleness the way a generated cache artifact is.
pub fn global_context_root(repo_key: &str) -> crate::Result<Utf8PathBuf> {
    Ok(global_ctx_root()?.join("context").join(repo_key))
}

/// Global context-ledger root for the current invocation's repository.
pub fn current_global_context_root() -> crate::Result<Utf8PathBuf> {
    global_context_root(&current_repo_key()?)
}

// ---------------------------------------------------------------------------
/// Machine-local index of known checkouts (P569). Generated, never authored.
const CHECKOUT_INDEX: &str = "checkouts.toml";

// checkout index: repository path + last-seen
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoIndexEntry {
    pub key: String,
    pub path: String,
    #[serde(rename = "last-seen-epoch")]
    pub last_seen_epoch: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RepoIndexDocument {
    #[serde(rename = "repo", default, skip_serializing_if = "Vec::is_empty")]
    repos: Vec<RepoIndexEntry>,
}

fn repo_index_path() -> crate::Result<Utf8PathBuf> {
    // P569: an index of checkouts — repo key -> path + last seen — so the
    // keyed `runs/<key>`, `cache/<key>` trees can point back at real working
    // copies. Named for what it indexes; `repos.toml` read as a list of
    // repositories you had configured, which it never was.
    Ok(global_ctx_root()?.join(CHECKOUT_INDEX))
}

fn read_repo_index_document(path: &Utf8Path) -> crate::Result<RepoIndexDocument> {
    match std::fs::read_to_string(path.as_std_path()) {
        Ok(text) => toml::from_str(&text).map_err(|source| {
            crate::parse::Error::TomlDecode {
                context: format!(
                    "decode repository index at {path}; fix or remove {path} to recover"
                ),
                source,
            }
            .into()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RepoIndexDocument::default()),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

fn write_repo_index_document(path: &Utf8Path, document: &RepoIndexDocument) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    let mut sorted = document.clone();
    sorted.repos.sort_by(|a, b| a.key.cmp(&b.key));
    let text =
        toml::to_string_pretty(&sorted).map_err(|source| crate::parse::Error::TomlEncode {
            context: format!("encode repository index at {path}"),
            source,
        })?;
    let file_name = path.file_name().unwrap_or(CHECKOUT_INDEX);
    let tmp_path = path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    std::fs::write(tmp_path.as_std_path(), text).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: tmp_path.to_string(),
            source,
        }
    })?;
    if let Err(source) = std::fs::rename(tmp_path.as_std_path(), path.as_std_path()) {
        let _ = std::fs::remove_file(tmp_path.as_std_path());
        return Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into());
    }
    Ok(())
}

/// Read the persisted repository index, sorted by key.
pub fn read_repo_index() -> crate::Result<Vec<RepoIndexEntry>> {
    let path = repo_index_path()?;
    Ok(read_repo_index_document(&path)?.repos)
}

/// Record (or refresh) the current repository's canonical path and
/// last-seen time in `checkouts.toml`, under an exclusive `flock` on a sibling
/// lock file so two runs starting at once never lose each other's entry
/// (reuses [`crate::file_lock`], the same primitive [`crate::merge_lock`]
/// and [`crate::builtin_store`] use for cross-process serialization).
/// Returns the repo key that was touched.
pub fn touch_repo_index() -> crate::Result<String> {
    let invocation = discover_invocation_root()?;
    let canonical = canonical_repo_root(invocation.path())?;
    let key = current_repo_key()?;
    let path = repo_index_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    let lock_path = path.with_file_name(format!("{}.lock", CHECKOUT_INDEX));
    let lock_file = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    crate::file_lock::lock_exclusive_blocking(&lock_file).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    // `lock_file`'s exclusive flock releases when it drops at the end of
    // this function, after the atomic rename below has landed.
    let mut document = read_repo_index_document(&path)?;
    let now = epoch_seconds();
    match document.repos.iter_mut().find(|entry| entry.key == key) {
        Some(entry) => {
            entry.path = canonical.to_string();
            entry.last_seen_epoch = now;
        }
        None => document.repos.push(RepoIndexEntry {
            key: key.clone(),
            path: canonical.to_string(),
            last_seen_epoch: now,
        }),
    }
    write_repo_index_document(&path, &document)?;
    Ok(key)
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
