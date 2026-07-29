use ctx_traits_core::digest::Digest;
use ctx_traits_core::import::plan::{RemoteFetchMethod, RemoteSourceEvidence, RemoteSourceKind};

/// Result of attempting a remote source fetch.
#[derive(Debug, Clone)]
pub struct RemoteFetchResult {
    /// Source evidence if successful.
    pub evidence: Option<RemoteSourceEvidence>,
    /// Error message if fetch failed or is unsupported.
    pub error: Option<String>,
}

/// Attempt to fetch a remote source.
///
/// Returns `RemoteFetchResult` with structured source evidence and
/// manual-download guidance when no HTTP client is available. This function
/// does not perform network IO; it classifies the URL and returns appropriate
/// structured evidence so the CLI can emit JSON output rather than plain
/// command errors.
pub fn fetch_remote_source(source_url: &str) -> RemoteFetchResult {
    let components = ctx_traits_core::import::plan::parse_github_url(source_url);

    if source_url.starts_with("https://github.com/")
        || source_url.starts_with("https://raw.githubusercontent.com/")
    {
        let evidence = build_github_evidence(source_url, components.as_ref());
        return RemoteFetchResult {
            evidence: Some(evidence.clone()),
            error: Some(format!(
                "GitHub remote import requires network IO which is not configured in this build. \
                 Manually download {source_url} and import locally, or configure a remote fetch adapter. \
                 Source kind: github, fetch method: {:?}, requested ref: {}",
                evidence.fetch_method, evidence.requested_ref,
            )),
        };
    }
    if source_url.starts_with("github:") {
        let normalized = source_url.strip_prefix("github:").unwrap_or(source_url);
        let canonical_url = format!("https://github.com/{normalized}");
        let normalized_components = ctx_traits_core::import::plan::parse_github_url(&canonical_url);
        let mut evidence = build_github_evidence(&canonical_url, normalized_components.as_ref());
        evidence.original_url = source_url.to_string();
        return RemoteFetchResult {
            evidence: Some(evidence.clone()),
            error: Some(format!(
                "github: shorthand remote import requires network IO which is not configured. \
                 Normalized URL: {canonical_url}. \
                 Manually download and import locally, or configure a remote fetch adapter. \
                 Source kind: github, fetch method: {:?}, requested ref: {}",
                evidence.fetch_method, evidence.requested_ref,
            )),
        };
    }
    if source_url.starts_with("https://www.skills.sh/")
        || source_url.starts_with("https://skills.sh/")
    {
        let evidence = RemoteSourceEvidence {
            kind: RemoteSourceKind::SkillsSh,
            original_url: source_url.to_string(),
            canonical_url: source_url.to_string(),
            resolved_commit: None,
            requested_ref: "none".to_string(),
            is_floating: false,
            source_set_digest: Digest::source(source_url),
            fetch_method: RemoteFetchMethod::Manual,
            redirect_chain: Vec::new(),
            warnings: vec![
                "skills.sh remote import is not configured; manual download required".to_string(),
            ],
        };
        return RemoteFetchResult {
            evidence: Some(evidence.clone()),
            error: Some(format!(
                "skills.sh remote import is unsupported; manually download {source_url} and import locally. \
                 Source kind: skills-sh, fetch method: {:?}",
                evidence.fetch_method,
            )),
        };
    }
    let evidence = RemoteSourceEvidence {
        kind: RemoteSourceKind::Manual,
        original_url: source_url.to_string(),
        canonical_url: source_url.to_string(),
        resolved_commit: None,
        requested_ref: "none".to_string(),
        is_floating: false,
        source_set_digest: Digest::source(source_url),
        fetch_method: RemoteFetchMethod::Manual,
        redirect_chain: Vec::new(),
        warnings: vec![format!("unsupported remote source: {source_url}")],
    };
    RemoteFetchResult {
        evidence: Some(evidence),
        error: Some(format!(
            "unsupported remote source: {source_url}; manually download and import locally"
        )),
    }
}

fn build_github_evidence(
    source_url: &str,
    components: Option<&ctx_traits_core::import::plan::GithubSourceComponents>,
) -> RemoteSourceEvidence {
    let ambiguous_ref_or_path = components.is_some_and(|c| {
        matches!(c.url_type.as_deref(), Some("blob" | "tree" | "raw"))
            && c.path.as_deref().is_some_and(|path| path.contains('/'))
    });
    let (canonical_url, requested_ref, is_floating) = match components {
        Some(c) => {
            let canonical = format!("https://github.com/{}/{}", c.owner, c.repo);
            let git_ref = if ambiguous_ref_or_path {
                "ambiguous-ref-or-path".to_string()
            } else {
                c.git_ref.clone().unwrap_or_else(|| "HEAD".to_string())
            };
            let floating = !is_likely_commit_sha(&git_ref);
            (canonical, git_ref, floating)
        }
        None => (source_url.to_string(), "HEAD".to_string(), true),
    };

    let mut warnings = Vec::new();
    if is_floating {
        warnings.push(format!(
            "floating ref '{requested_ref}'; resolved commit SHA not available without network IO"
        ));
    }
    if ambiguous_ref_or_path {
        warnings.push(
            "unsupported-source: GitHub blob/tree/raw URL has multi-segment ref/path ambiguity; \
             requested ref is not resolved without GitHub API or network IO"
                .to_string(),
        );
    }

    RemoteSourceEvidence {
        kind: RemoteSourceKind::Github,
        original_url: source_url.to_string(),
        canonical_url,
        resolved_commit: None,
        requested_ref,
        is_floating,
        source_set_digest: Digest::source(source_url),
        fetch_method: RemoteFetchMethod::Manual,
        redirect_chain: Vec::new(),
        warnings,
    }
}

fn is_likely_commit_sha(s: &str) -> bool {
    s.len() >= 7 && s.len() <= 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}
