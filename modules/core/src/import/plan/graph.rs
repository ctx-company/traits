/// Import dependency graph planning.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SkillFileConversionRole {
    /// Entry `SKILL.md` file.
    Entry,
    /// Converted to a canonical resource.
    Resource,
    /// Preserved as ancillary evidence only.
    Ancillary,
}

/// How an artifact was discovered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ArtifactDiscovery {
    /// Entry file (SKILL.md).
    EntryFile,
    /// Referenced by a safe relative Markdown link.
    LinkedFrom { source_path: String },
    /// Explicitly included via flag.
    ExplicitInclude,
    /// Profile manifest rule.
    ProfileRule,
}

/// Status of a link/edge in the artifact graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SkillLinkStatus {
    /// Successfully included.
    Included,
    /// External link skipped.
    SkippedExternal,
    /// Target not found.
    Missing,
    /// Unsafe path or symlink.
    Unsafe,
    /// Unsupported link syntax.
    UnsupportedSyntax,
}

/// One node in the skill artifact graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SkillArtifactNode {
    /// Normalized relative path.
    pub path: String,
    /// File digest.
    pub digest: Digest,
    /// Media type guess.
    pub media_type: String,
    /// How the node was discovered.
    pub discovered_by: ArtifactDiscovery,
    /// Conversion role.
    pub conversion_role: SkillFileConversionRole,
}

/// One edge (link/reference) in the skill artifact graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SkillArtifactEdge {
    /// Source file path.
    pub source_path: String,
    /// Target path or URI.
    pub target_path: String,
    /// Link text from the Markdown link.
    pub link_text: String,
    /// Edge status.
    pub status: SkillLinkStatus,
}

/// Deterministic artifact graph for a multi-file skill package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct SkillArtifactGraph {
    /// Graph nodes (artifacts).
    pub nodes: Vec<SkillArtifactNode>,
    /// Graph edges (links/references).
    pub edges: Vec<SkillArtifactEdge>,
    /// Deterministic digest of the graph structure.
    pub graph_digest: Digest,
}

/// Resource ID mapping from supporting file path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ResourceIdMapping {
    /// Source file path.
    pub source_path: String,
    /// Canonical resource ID.
    pub resource_id: String,
    /// Resource variant.
    pub variant: String,
}

/// A raw Markdown link discovered in source text.
///
/// The IO traversal layer classifies each link into `Included`,
/// `SkippedExternal`, `Missing`, `Unsafe`, or `UnsupportedSyntax`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RawMarkdownLink {
    /// Raw target string as it appeared in the Markdown.
    pub raw_target: String,
    /// Link text between `[` and `]`.
    pub link_text: String,
    /// Resolved relative path from the current file's parent directory.
    /// For `SKILL.md` linking `checklist.md`, this is `checklist.md`.
    /// For `docs/guide.md` linking `more.md`, this is `docs/more.md`.
    pub resolved_path: String,
}

/// Discover all Markdown links from text content without filtering.
///
/// Returns raw link data including external, absolute, fragment-only, and
/// unsafe links. The IO traversal layer is responsible for classifying each
/// link into `Included`, `SkippedExternal`, `Missing`, `Unsafe`, or
/// `UnsupportedSyntax`.
///
/// Relative paths are resolved against the **parent directory** of
/// `current_file_path`, not by appending to the file name.
pub fn discover_markdown_links(text: &str, current_file_path: &str) -> Vec<RawMarkdownLink> {
    let parent_dir = match current_file_path.rfind('/') {
        Some(idx) => &current_file_path[..idx],
        None => "",
    };

    let mut links = Vec::new();
    for line in text.lines() {
        let mut search_from = 0;
        while let Some(bracket_open) = line[search_from..].find('[') {
            let bracket_open = search_from + bracket_open;
            let after_bracket = &line[bracket_open + 1..];
            let Some(close_offset) = after_bracket.find("](") else {
                search_from = bracket_open + 1;
                continue;
            };
            let link_text = &after_bracket[..close_offset];
            let target_start = bracket_open + 1 + close_offset + 2;
            let rest = &line[target_start..];
            let Some(end) = rest.find(')') else {
                search_from = target_start;
                continue;
            };
            let target = &rest[..end];
            let resolved_path = resolve_relative_to_parent(parent_dir, target);
            links.push(RawMarkdownLink {
                raw_target: target.to_string(),
                link_text: link_text.to_string(),
                resolved_path,
            });
            search_from = target_start + end + 1;
        }
    }
    links
}

fn resolve_relative_to_parent(parent_dir: &str, target: &str) -> String {
    if parent_dir.is_empty() {
        target.to_string()
    } else {
        format!("{parent_dir}/{target}")
    }
}

#[cfg(test)]
mod tests {
    use super::discover_markdown_links;

    #[test]
    fn discovers_raw_links_and_resolves_from_the_parent_directory() {
        let links = discover_markdown_links(
            "[guide](more.md) [site](https://example.com) [section](#details)",
            "docs/guide.md",
        );

        assert_eq!(links.len(), 3);
        assert_eq!(links[0].raw_target, "more.md");
        assert_eq!(links[0].resolved_path, "docs/more.md");
        assert_eq!(links[1].raw_target, "https://example.com");
        assert_eq!(links[2].raw_target, "#details");
    }
}

/// Derive a canonical resource ID from a relative file path.
///
/// Includes parent directory components to avoid collisions between files
/// with the same basename in different directories. For example,
/// `docs/checklist.md` produces `docs-checklist`, while `checklist.md`
/// produces `checklist`.
pub fn resource_id_from_path(path: &str) -> String {
    let no_ext = path
        .strip_suffix(".md")
        .or_else(|| path.strip_suffix(".MD"))
        .unwrap_or(path);
    let slug: String = no_ext
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "imported-resource".to_string()
    } else {
        slug.to_string()
    }
}

/// Compute a deterministic graph digest from nodes and edges.
pub fn compute_graph_digest(nodes: &[SkillArtifactNode], edges: &[SkillArtifactEdge]) -> Digest {
    let mut sorted_nodes: Vec<&SkillArtifactNode> = nodes.iter().collect();
    sorted_nodes.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seed = String::new();
    for node in &sorted_nodes {
        seed.push_str(&node.path);
        seed.push(':');
        seed.push_str(node.digest.as_str());
        seed.push('\n');
    }
    let mut sorted_edges: Vec<&SkillArtifactEdge> = edges.iter().collect();
    sorted_edges
        .sort_by(|a, b| (&a.source_path, &a.target_path).cmp(&(&b.source_path, &b.target_path)));
    for edge in &sorted_edges {
        seed.push_str(&edge.source_path);
        seed.push_str("->");
        seed.push_str(&edge.target_path);
        seed.push('\n');
    }
    Digest::source(&seed)
}

// ---------------------------------------------------------------------------
// P93: Remote source evidence
// ---------------------------------------------------------------------------
