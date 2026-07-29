use std::fs;

use camino::{Utf8Component, Utf8Path};
use ctx_traits_core::digest::Digest;

pub(super) const MAX_INLINE_SIZE: u64 = 1_048_576;

pub(super) fn read_regular_utf8_file(path: &Utf8Path) -> crate::Result<String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return filesystem_error(path, "import source file must not be a symlink");
            }
            if !metadata.file_type().is_file() {
                return filesystem_error(path, "import source file must be a regular file");
            }
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: e,
            }
            .into());
        }
    }

    Ok(
        fs::read_to_string(path).map_err(|e| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        })?,
    )
}

pub(super) fn digest_regular_import_file(path: &Utf8Path) -> crate::Result<Digest> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Ok(Digest::source("symlink-import-path"));
            }
            if !metadata.file_type().is_file() {
                return Ok(Digest::source("special-import-path"));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Digest::source("missing-import-path"));
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: e,
            }
            .into());
        }
    }

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Digest::source("missing-import-path"));
        }
        Err(e) => {
            return Err(crate::environment::Error::Filesystem {
                path: path.to_string(),
                source: e,
            }
            .into());
        }
    };

    Ok(Digest::from_bytes(&bytes))
}

pub(super) fn is_safe_relative_path(relative: &str) -> bool {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.contains('\\')
        || (relative.len() >= 2 && relative.as_bytes()[1] == b':')
    {
        return false;
    }

    Utf8Path::new(relative)
        .components()
        .all(|component| matches!(component, Utf8Component::Normal(_)))
}

pub(super) fn validate_path_shape(path: &Utf8Path) -> crate::Result<()> {
    let raw = path.as_str();
    if raw.is_empty() || raw.contains('\\') || raw.contains("//") {
        return filesystem_error(path, "import path contains an unsafe path shape");
    }
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return filesystem_error(path, "import path must not use a Windows drive prefix");
    }
    for component in path.components() {
        match component {
            Utf8Component::ParentDir => {
                return filesystem_error(path, "import path must not contain '..' traversal");
            }
            Utf8Component::Normal("") => {
                return filesystem_error(path, "import path must not contain empty path segments");
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn reject_existing_symlink_ancestors(path: &Utf8Path) -> crate::Result<()> {
    let mut ancestors = Vec::new();
    let mut current = Some(path);
    while let Some(path) = current {
        ancestors.push(path);
        current = path.parent();
    }

    let mut skipped_absolute_alias = false;
    for ancestor in ancestors.into_iter().rev() {
        if ancestor.as_str().is_empty() {
            continue;
        }
        if path.is_absolute()
            && !skipped_absolute_alias
            && ancestor
                .parent()
                .is_some_and(|parent| parent.as_str() == "/")
        {
            skipped_absolute_alias = true;
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return filesystem_error(
                        ancestor,
                        "import path ancestor must not be a symlink",
                    );
                }
                if !metadata.file_type().is_dir() {
                    return filesystem_error(ancestor, "import path ancestor must be a directory");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(crate::environment::Error::Filesystem {
                    path: ancestor.to_string(),
                    source: e,
                }
                .into());
            }
        }
    }
    Ok(())
}

pub(super) fn validate_generated_leaf(path: &Utf8Path, allow_overwrite: bool) -> crate::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return filesystem_error(path, "generated import target is not a regular file");
            }
            if !allow_overwrite {
                return filesystem_error(path, "generated import target already exists");
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        }
        .into()),
    }
}

pub(super) fn relative_import_path(base: &Utf8Path, path: &Utf8Path) -> crate::Result<String> {
    Ok(path
        .strip_prefix(base)
        .map(|relative| relative.to_string())
        .map_err(|_| crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "import source path escaped its base directory",
            ),
        })?)
}

pub(super) fn digest_entries(mut entries: Vec<(String, Digest)>) -> Digest {
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut text = String::from("import-source\n");
    for (path, digest) in entries {
        text.push_str(&path);
        text.push('\n');
        text.push_str(digest.as_str());
        text.push('\n');
    }

    Digest::source(&text)
}

pub(super) fn is_likely_utf8(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    std::str::from_utf8(bytes).is_ok()
}

pub(super) fn guess_media(path: &str, is_text: bool) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with("skill.md") {
        "skill-md".to_string()
    } else if lower.ends_with(".md") {
        "markdown".to_string()
    } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        "yaml-frontmatter".to_string()
    } else if !is_text {
        "resource".to_string()
    } else {
        "unknown".to_string()
    }
}

pub(super) fn filesystem_error<T>(path: &Utf8Path, message: &str) -> crate::Result<T> {
    Err(crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message),
    }
    .into())
}

const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(super) fn encode_base64(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        output.push(BASE64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        output.push(BASE64_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            output.push(BASE64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(BASE64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}
