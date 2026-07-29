/// Defines remote sources for import planning.
/// Remote import planning.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum RemoteSourceKind {
    /// GitHub repository or file.
    Github,
    /// skills.sh package.
    SkillsSh,
    /// HTTPS archive download.
    HttpsArchive,
    /// HTTPS single file.
    HttpsFile,
    /// Manually downloaded.
    Manual,
}

/// Fetch method used to retrieve remote artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum RemoteFetchMethod {
    /// GitHub API or archive.
    Api,
    /// Archive download.
    Archive,
    /// Raw file download.
    RawFile,
    /// Manual download by user.
    Manual,
}

/// Evidence for a remote import source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RemoteSourceEvidence {
    /// Source kind.
    pub kind: RemoteSourceKind,
    /// Original URL or input string.
    pub original_url: String,
    /// Normalized canonical URL.
    pub canonical_url: String,
    /// Resolved commit SHA or immutable version when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
    /// Requested ref and whether it is floating.
    pub requested_ref: String,
    /// Whether the ref is floating (branch/tag vs commit SHA).
    pub is_floating: bool,
    /// Content/source-set digest.
    pub source_set_digest: Digest,
    /// Fetch method used.
    pub fetch_method: RemoteFetchMethod,
    /// Redirect chain host evidence if redirects were followed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirect_chain: Vec<String>,
    /// Warnings for auth requirements, rate limits, floating refs, etc.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Classify a source string as local or remote.
pub fn classify_source(source: &str) -> ImportSource {
    if source.starts_with("https://github.com/")
        || source.starts_with("https://raw.githubusercontent.com/")
        || source.starts_with("github:")
        || source.starts_with("https://www.skills.sh/")
        || source.starts_with("https://skills.sh/")
    {
        ImportSource::Git {
            url: source.to_string(),
        }
    } else {
        ImportSource::Local {
            path: source.to_string(),
        }
    }
}

/// Parsed components of a GitHub source URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct GithubSourceComponents {
    /// Repository owner.
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// URL type: `blob`, `tree`, or `raw` (for raw.githubusercontent.com).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_type: Option<String>,
    /// Git ref: branch name, tag, or commit SHA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Path within the repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Parse a GitHub URL into its components if recognizable.
///
/// Handles:
/// - `https://github.com/<owner>/<repo>/blob/<ref>/<path>/SKILL.md`
/// - `https://github.com/<owner>/<repo>/tree/<ref>/<path>`
/// - `https://raw.githubusercontent.com/<owner>/<repo>/<ref>/<path>/SKILL.md`
pub fn parse_github_url(url: &str) -> Option<GithubSourceComponents> {
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.splitn(5, '/').collect();
        if parts.len() >= 2 {
            let owner = parts[0].to_string();
            let repo = parts[1].to_string();
            if parts.len() >= 4 && (parts[2] == "blob" || parts[2] == "tree") {
                let url_type = parts[2].to_string();
                let git_ref = parts[3].to_string();
                let path = if parts.len() >= 5 {
                    Some(parts[4].to_string())
                } else {
                    None
                };
                return Some(GithubSourceComponents {
                    owner,
                    repo,
                    url_type: Some(url_type),
                    git_ref: Some(git_ref),
                    path,
                });
            }
            return Some(GithubSourceComponents {
                owner,
                repo,
                url_type: None,
                git_ref: None,
                path: parts.get(3).map(|s| s.to_string()),
            });
        }
    }
    if let Some(rest) = url.strip_prefix("https://raw.githubusercontent.com/") {
        let parts: Vec<&str> = rest.splitn(4, '/').collect();
        if parts.len() >= 4 {
            return Some(GithubSourceComponents {
                owner: parts[0].to_string(),
                repo: parts[1].to_string(),
                url_type: Some("raw".to_string()),
                git_ref: Some(parts[2].to_string()),
                path: Some(parts[3].to_string()),
            });
        }
    }
    None
}
