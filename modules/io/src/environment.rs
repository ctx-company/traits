//! Environment side-effect errors.
//!
//! These errors come from filesystem, process, Git, or host-install effects.

use thiserror::Error;

/// A known free-space reading below a configured dispatch floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskFullObservation {
    pub available_bytes: u64,
    pub probed_path: camino::Utf8PathBuf,
}

/// Return the execution (or invocation) volume plus the repository's global
/// cache volume. Failed repository discovery is deliberately lenient: callers
/// still get the known execution volume, but never park on an invented cache.
pub fn dispatch_disk_probe_paths(
    execution_dir: Option<&camino::Utf8Path>,
    repo_path: Option<&camino::Utf8Path>,
) -> Vec<camino::Utf8PathBuf> {
    let invocation = execution_dir
        .map(ToOwned::to_owned)
        .or_else(|| repo_path.map(ToOwned::to_owned))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|path| camino::Utf8PathBuf::from_path_buf(path).ok())
        });
    let mut paths = invocation.iter().cloned().collect::<Vec<_>>();
    let repo_root = repo_path.map(ToOwned::to_owned).or_else(|| {
        invocation
            .as_deref()
            .and_then(|path| crate::repository::discover_main_repo_root(path).ok())
    });
    if let Some(repo_root) = repo_root
        && let Ok(canonical) = crate::state::canonical_repo_root(&repo_root)
        && let Ok(cache_root) = crate::state::global_cache_root(&crate::state::repo_key(&canonical))
    {
        paths.push(cache_root);
    }
    paths
}

/// Apply the shared dispatch-volume policy to a configured free-space floor.
pub fn dispatch_disk_observation(
    execution_dir: Option<&camino::Utf8Path>,
    repo_path: Option<&camino::Utf8Path>,
    floor_mb: u64,
) -> Option<DiskFullObservation> {
    if floor_mb == 0 {
        return None;
    }
    let floor_bytes = floor_mb.saturating_mul(1024 * 1024);
    dispatch_disk_available(execution_dir, repo_path)
        .filter(|observation| observation.available_bytes < floor_bytes)
}

/// Return the lowest known free-space observation across the execution and
/// cache volumes. Unlike [`dispatch_disk_observation`], this reports known
/// space regardless of a configured floor so ENOSPC evidence does not turn a
/// successful probe into an unknown value.
pub fn dispatch_disk_available(
    execution_dir: Option<&camino::Utf8Path>,
    repo_path: Option<&camino::Utf8Path>,
) -> Option<DiskFullObservation> {
    let paths = dispatch_disk_probe_paths(execution_dir, repo_path);
    let refs = paths
        .iter()
        .map(camino::Utf8PathBuf::as_path)
        .collect::<Vec<_>>();
    observed_available_bytes(&refs)
}

/// Return the lowest known free-space observation. Unknown probes are
/// deliberately ignored: a park must be evidence-based.
pub fn observed_available_bytes(paths: &[&camino::Utf8Path]) -> Option<DiskFullObservation> {
    let mut lowest = None;
    let mut probed = std::collections::BTreeSet::new();
    for path in paths {
        let mut ancestor = (*path).to_owned();
        while !ancestor.exists() {
            if !ancestor.pop() {
                break;
            }
        }
        if !ancestor.exists() || !probed.insert(ancestor.clone()) {
            continue;
        }
        let Ok(Some(available_bytes)) = available_disk_bytes(&ancestor) else {
            continue;
        };
        if lowest
            .as_ref()
            .is_none_or(|current: &DiskFullObservation| available_bytes < current.available_bytes)
        {
            lowest = Some(DiskFullObservation {
                available_bytes,
                probed_path: ancestor,
            });
        }
    }
    lowest
}

/// Return the lowest known free-space observation when it is below `floor_mb`.
pub fn observed_available_below_floor(
    paths: &[&camino::Utf8Path],
    floor_mb: u64,
) -> Option<DiskFullObservation> {
    if floor_mb == 0 {
        return None;
    }
    let floor_bytes = floor_mb.saturating_mul(1024 * 1024);
    observed_available_bytes(paths).filter(|observation| observation.available_bytes < floor_bytes)
}

/// Whether an error chain contains a disk-full or quota-full OS error.
pub fn error_chain_is_disk_full(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if let Some(io) = candidate.downcast_ref::<std::io::Error>()
            && (io.raw_os_error() == Some(libc::ENOSPC)
                || matches!(
                    io.kind(),
                    std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
                ))
        {
            return true;
        }
        current = candidate.source();
    }
    false
}

/// Render a filesystem error, appending an actionable next step when the
/// underlying `io::Error` is a plain "not found": otherwise the message
/// names the failing path but leaves the reader to guess whether to fix a
/// typo, create the file, or check permissions.
fn format_filesystem_error(path: &str, source: &std::io::Error) -> String {
    if source.kind() == std::io::ErrorKind::NotFound {
        format!(
            "filesystem error at {path}: {source}; check the path for a typo, or create the file before retrying"
        )
    } else {
        format!("filesystem error at {path}: {source}")
    }
}

/// Shared `Display` for `Git`/`Process`: command + exit code, path when
/// present, and the timeout named only when it actually fired. Keeps the
/// two symmetric variants from drifting apart.
fn format_command_error(
    kind: &str,
    command: &Option<String>,
    path: &Option<String>,
    exit_status: Option<i32>,
    timed_out: bool,
    message: &str,
) -> String {
    let mut out = match command {
        Some(command) => format!("{kind} failed: {command}"),
        None => format!("{kind} failed"),
    };
    match exit_status {
        Some(code) => out.push_str(&format!(" (exit {code})")),
        None => out.push_str(" (no exit code)"),
    }
    if let Some(path) = path {
        out.push_str(&format!(" in {path}"));
    }
    if timed_out {
        out.push_str(" — timed out");
    }
    if !message.is_empty() {
        out.push_str(&format!(" — {message}"));
    }
    out
}

/// Failures while interacting with the host environment.
#[derive(Debug, Error)]
pub enum Error {
    // Filesystem effects.
    #[error("{}", format_filesystem_error(path, source))]
    Filesystem {
        path: String,
        #[source]
        source: std::io::Error,
    },

    // External command effects.
    #[error("{}", format_command_error("git", command, path, *exit_status, *timed_out, message))]
    Git {
        command: Option<String>,
        path: Option<String>,
        exit_status: Option<i32>,
        timed_out: bool,
        message: String,
    },

    #[error("{}", format_command_error("command", command, path, *exit_status, *timed_out, message))]
    Process {
        command: Option<String>,
        path: Option<String>,
        exit_status: Option<i32>,
        timed_out: bool,
        message: String,
    },

    // Host installation effects.
    #[error("host install error: {message}")]
    HostInstall {
        host: Option<String>,
        path: Option<String>,
        message: String,
    },
}

/// Free bytes available on the volume holding `path` (P462 disk preflight).
/// `Ok(None)` on a platform where the probe is unavailable — a park decision
/// must never be based on an unknowable answer, so callers skip the check
/// rather than treat `None` as "below floor".
#[cfg(unix)]
pub fn available_disk_bytes(path: &camino::Utf8Path) -> crate::Result<Option<u64>> {
    use std::os::unix::ffi::OsStrExt;

    let c_path =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path contains a NUL byte",
            ),
        })?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if result != 0 {
        return Err(Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::last_os_error(),
        }
        .into());
    }
    Ok(Some(stat.f_bavail as u64 * stat.f_frsize as u64))
}

#[cfg(not(unix))]
pub fn available_disk_bytes(_path: &camino::Utf8Path) -> crate::Result<Option<u64>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_full_error_classifier_accepts_enospc_only() {
        let full = std::io::Error::from_raw_os_error(libc::ENOSPC);
        let other = std::io::Error::from_raw_os_error(libc::EIO);
        assert!(error_chain_is_disk_full(&full));
        assert!(!error_chain_is_disk_full(&other));
    }

    #[derive(Debug, thiserror::Error)]
    #[error("wrapped command failure")]
    struct WrappedError {
        #[source]
        source: std::io::Error,
    }

    #[test]
    fn disk_full_error_classifier_walks_wrapped_errors() {
        assert!(error_chain_is_disk_full(&WrappedError {
            source: std::io::Error::from_raw_os_error(libc::ENOSPC),
        }));
    }

    #[test]
    fn zero_floor_never_parks() {
        assert_eq!(
            observed_available_below_floor(&[camino::Utf8Path::new("/")], 0),
            None
        );
    }

    #[test]
    fn disk_full_unavailable_observation_is_none() {
        assert_eq!(observed_available_bytes(&[]), None);
    }

    #[cfg(unix)]
    #[test]
    fn disk_full_known_observation_is_retained_above_the_floor() {
        let path = camino::Utf8Path::new("/");
        let known = observed_available_bytes(&[path]).expect("the root volume is known on unix");
        assert_eq!(
            observed_available_below_floor(&[path], 1),
            (known.available_bytes < 1024 * 1024).then_some(known)
        );
    }

    #[cfg(unix)]
    #[test]
    fn nonexistent_candidate_uses_its_existing_ancestor() {
        let observation = observed_available_below_floor(
            &[camino::Utf8Path::new("/ctx-traits-disk-floor-test/missing")],
            u64::MAX,
        )
        .expect("the root volume is known on unix");
        assert_eq!(observation.probed_path, camino::Utf8Path::new("/"));
    }
}
