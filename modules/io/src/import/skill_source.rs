use std::collections::BTreeSet;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
use ctx_traits_core::import::plan::{
    ArtifactDiscovery, SkillArtifactEdge, SkillArtifactGraph, SkillArtifactNode,
    SkillFileConversionRole, SkillLinkStatus,
};

use super::support::{filesystem_error, guess_media, read_regular_utf8_file, relative_import_path};

/// A loaded multi-file skill package source.
#[derive(Debug, Clone)]
pub struct MultiFileSkillSource {
    /// Source root directory.
    pub source_root: Utf8PathBuf,
    /// Path to the entry `SKILL.md`.
    pub skill_path: Utf8PathBuf,
    /// `SKILL.md` text content.
    pub skill_markdown: String,
    /// Source directory name.
    pub source_name: String,
    /// Additional included files (path relative to source_root, content).
    pub included_files: Vec<(String, String)>,
    /// Artifact graph.
    pub graph: SkillArtifactGraph,
    /// Warnings from graph construction.
    pub graph_warnings: Vec<String>,
}

/// Read a multi-file skill directory, discovering linked files.
pub fn read_multi_file_skill_source(source_root: &Utf8Path) -> crate::Result<MultiFileSkillSource> {
    let skill_path = source_root.join("SKILL.md");
    let skill_markdown = read_regular_utf8_file(&skill_path)?;
    let source_name = source_root
        .file_name()
        .map_or("imported-skill", |name| name)
        .to_string();

    let mut included_files = Vec::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut warnings = Vec::new();
    let mut visited = BTreeSet::new();
    let mut state = TraversalState {
        included_files: &mut included_files,
        nodes: &mut nodes,
        edges: &mut edges,
        warnings: &mut warnings,
        visited: &mut visited,
    };

    traverse_linked_files(
        source_root,
        "SKILL.md",
        &skill_markdown,
        source_root,
        &mut state,
    )?;

    let entry_digest = Digest::source(&skill_markdown);
    nodes.push(SkillArtifactNode {
        path: "SKILL.md".to_string(),
        digest: entry_digest,
        media_type: "skill-md".to_string(),
        discovered_by: ArtifactDiscovery::EntryFile,
        conversion_role: SkillFileConversionRole::Entry,
    });

    let graph_digest = ctx_traits_core::import::plan::compute_graph_digest(&nodes, &edges);
    nodes.sort_by(|a, b| a.path.cmp(&b.path));
    edges.sort_by(|a, b| (&a.source_path, &a.target_path).cmp(&(&b.source_path, &b.target_path)));

    Ok(MultiFileSkillSource {
        source_root: source_root.to_path_buf(),
        skill_path,
        skill_markdown,
        source_name,
        included_files,
        graph: SkillArtifactGraph {
            nodes,
            edges,
            graph_digest,
        },
        graph_warnings: warnings,
    })
}

struct TraversalState<'a> {
    included_files: &'a mut Vec<(String, String)>,
    nodes: &'a mut Vec<SkillArtifactNode>,
    edges: &'a mut Vec<SkillArtifactEdge>,
    warnings: &'a mut Vec<String>,
    visited: &'a mut BTreeSet<String>,
}

fn traverse_linked_files(
    source_root: &Utf8Path,
    current_rel: &str,
    current_text: &str,
    base: &Utf8Path,
    state: &mut TraversalState<'_>,
) -> crate::Result<()> {
    if state.visited.contains(current_rel) {
        return Ok(());
    }
    state.visited.insert(current_rel.to_string());

    let links = ctx_traits_core::import::plan::discover_markdown_links(current_text, current_rel);
    for link in links {
        let resolved = &link.resolved_path;
        let full_path = base.join(resolved);
        let edge_status = check_link_target(source_root, &full_path, &link.raw_target);

        match edge_status {
            SkillLinkStatus::Included => {
                if state.visited.contains(resolved) {
                    state.edges.push(SkillArtifactEdge {
                        source_path: current_rel.to_string(),
                        target_path: resolved.clone(),
                        link_text: link.link_text,
                        status: SkillLinkStatus::Included,
                    });
                    continue;
                }
                let content = match read_regular_utf8_file(&full_path) {
                    Ok(text) => text,
                    Err(_) => {
                        state
                            .warnings
                            .push(format!("could not read linked file: {resolved}"));
                        state.edges.push(SkillArtifactEdge {
                            source_path: current_rel.to_string(),
                            target_path: resolved.clone(),
                            link_text: link.link_text,
                            status: SkillLinkStatus::Missing,
                        });
                        continue;
                    }
                };
                let digest = Digest::source(&content);
                let media = guess_media(resolved, true);
                state
                    .included_files
                    .push((resolved.clone(), content.clone()));
                state.nodes.push(SkillArtifactNode {
                    path: resolved.clone(),
                    digest,
                    media_type: media,
                    discovered_by: ArtifactDiscovery::LinkedFrom {
                        source_path: current_rel.to_string(),
                    },
                    conversion_role: SkillFileConversionRole::Resource,
                });
                state.edges.push(SkillArtifactEdge {
                    source_path: current_rel.to_string(),
                    target_path: resolved.clone(),
                    link_text: link.link_text,
                    status: SkillLinkStatus::Included,
                });
                traverse_linked_files(source_root, resolved, &content, base, state)?;
            }
            status => {
                state.edges.push(SkillArtifactEdge {
                    source_path: current_rel.to_string(),
                    target_path: link.raw_target.clone(),
                    link_text: link.link_text,
                    status,
                });
            }
        }
    }
    Ok(())
}

/// One accepted doctor candidate file, already read as UTF-8 text.
///
/// `ctx traits doctor` reads every candidate exactly once here so its
/// analysis (in `ctx-traits-core`) never has to touch the filesystem again.
#[derive(Debug, Clone)]
pub struct DoctorSourceFile {
    /// Path relative to the doctor root, forward-slash separated.
    pub relative_path: String,
    pub content: String,
}

/// A per-file discovery or read failure that does not abort the walk.
#[derive(Debug, Clone)]
pub struct DoctorDiscoveryError {
    /// Path relative to the doctor root, forward-slash separated.
    pub relative_path: String,
    pub message: String,
}

/// Discovery result for `ctx traits doctor`: every accepted skill file found
/// under a root, sorted by relative path, plus per-file errors for entries
/// that could not be read safely.
#[derive(Debug, Clone, Default)]
pub struct DoctorDiscovery {
    /// The directory `files`/`errors` paths are relative to. Equal to the
    /// requested root when it is a directory; equal to that file's parent
    /// directory when the requested root is itself a single accepted file
    /// (so `effective_root`/`relative_path` always join back into a path
    /// usable from the caller's own working directory).
    pub effective_root: Utf8PathBuf,
    pub files: Vec<DoctorSourceFile>,
    pub errors: Vec<DoctorDiscoveryError>,
}

const DOCTOR_ACCEPTED_FILENAMES: [&str; 3] = ["SKILL.md", "AGENTS.md", "CLAUDE.md"];
const DOCTOR_SKIPPED_DIR_NAMES: [&str; 3] = [".git", "node_modules", "target"];

/// Discover `ctx traits doctor` candidate files under `root`.
///
/// `root` may be a single accepted file (`SKILL.md`, `AGENTS.md`, or
/// `CLAUDE.md`) or a directory to walk recursively. Symlinked files and
/// directories are never followed (checked via `symlink_metadata`/
/// `DirEntry::file_type`, never `canonicalize`); `.git`, `node_modules`, and
/// `target` directories are skipped. Individual unreadable or non-UTF-8 files
/// are recorded in `errors` and do not abort the walk; a root path that
/// cannot be listed at all is a hard error.
pub fn discover_doctor_sources(root: &Utf8Path) -> crate::Result<DoctorDiscovery> {
    let root_metadata =
        fs::symlink_metadata(root).map_err(|e| crate::environment::Error::Filesystem {
            path: root.to_string(),
            source: e,
        })?;

    if root_metadata.file_type().is_symlink() {
        return filesystem_error(root, "doctor root must not be a symlink");
    }

    if root_metadata.file_type().is_file() {
        let Some(name) = root.file_name() else {
            return filesystem_error(root, "doctor root file has no file name");
        };
        if !DOCTOR_ACCEPTED_FILENAMES.contains(&name) {
            return filesystem_error(
                root,
                "doctor root file must be named SKILL.md, AGENTS.md, or CLAUDE.md",
            );
        }
        let mut discovery = DoctorDiscovery {
            effective_root: root
                .parent()
                .map_or_else(|| Utf8PathBuf::from("."), Utf8Path::to_path_buf),
            ..DoctorDiscovery::default()
        };
        match read_regular_utf8_file(root) {
            Ok(content) => discovery.files.push(DoctorSourceFile {
                relative_path: name.to_string(),
                content,
            }),
            Err(e) => discovery.errors.push(DoctorDiscoveryError {
                relative_path: name.to_string(),
                message: e.to_string(),
            }),
        }
        return Ok(discovery);
    }

    if !root_metadata.file_type().is_dir() {
        return filesystem_error(root, "doctor root must be a regular file or directory");
    }

    let mut discovery = DoctorDiscovery {
        effective_root: root.to_path_buf(),
        ..DoctorDiscovery::default()
    };
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read_dir = match fs::read_dir(&dir) {
            Ok(read_dir) => read_dir,
            Err(e) => {
                discovery.errors.push(DoctorDiscoveryError {
                    relative_path: relative_import_path(root, &dir)
                        .unwrap_or_else(|_| dir.to_string()),
                    message: format!("could not list directory: {e}"),
                });
                continue;
            }
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    discovery.errors.push(DoctorDiscoveryError {
                        relative_path: relative_import_path(root, &dir)
                            .unwrap_or_else(|_| dir.to_string()),
                        message: format!("could not read directory entry: {e}"),
                    });
                    continue;
                }
            };
            let Ok(entry_path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if let Some(name) = entry_path.file_name()
                    && DOCTOR_SKIPPED_DIR_NAMES.contains(&name)
                {
                    continue;
                }
                stack.push(entry_path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = entry_path.file_name() else {
                continue;
            };
            if !DOCTOR_ACCEPTED_FILENAMES.contains(&name) {
                continue;
            }
            let relative_path =
                relative_import_path(root, &entry_path).unwrap_or_else(|_| entry_path.to_string());
            match read_regular_utf8_file(&entry_path) {
                Ok(content) => discovery.files.push(DoctorSourceFile {
                    relative_path,
                    content,
                }),
                Err(e) => discovery.errors.push(DoctorDiscoveryError {
                    relative_path,
                    message: e.to_string(),
                }),
            }
        }
    }

    discovery
        .files
        .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    discovery
        .errors
        .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    Ok(discovery)
}

fn check_link_target(
    source_root: &Utf8Path,
    full_path: &Utf8Path,
    target_path: &str,
) -> SkillLinkStatus {
    if target_path.starts_with("http://") || target_path.starts_with("https://") {
        return SkillLinkStatus::SkippedExternal;
    }
    if target_path.starts_with('/') || target_path.contains("..") {
        return SkillLinkStatus::Unsafe;
    }
    match fs::symlink_metadata(full_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return SkillLinkStatus::Unsafe;
            }
            if !metadata.file_type().is_file() {
                return SkillLinkStatus::Unsafe;
            }
            if !full_path.starts_with(source_root) {
                return SkillLinkStatus::Unsafe;
            }
            SkillLinkStatus::Included
        }
        Err(_) => SkillLinkStatus::Missing,
    }
}
