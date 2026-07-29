//! Environment side-effect errors.
//!
//! These errors come from filesystem, process, Git, or host-install effects.

use thiserror::Error;

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
