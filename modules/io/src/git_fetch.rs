//! GitHub-shaped git transport: ref resolution over the smart-HTTP
//! `info/refs` protocol and content fetch over GitHub codeload tarballs
//! (task 0191, phases a-b).
//!
//! No `git` or `node` process is ever spawned on this consume path: ref
//! resolution is a pkt-line parse of one HTTP response, content fetch is a
//! tarball download reusing the same hardened extraction walk the npm
//! registry client uses ([`crate::registry::extract_verified_tarball_with_prefix`]).

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};

/// Default GitHub codeload host, overridable via
/// `CTX_TRAITS_GIT_CODELOAD_BASE` so scratch fixtures can serve tarballs
/// locally — mirrors [`crate::distribution::resolve_registry_base_with_source`]'s
/// env-override precedence.
pub const DEFAULT_CODELOAD_BASE: &str = "https://codeload.github.com";

/// git transport failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git request to {url} failed: {message}")]
    Request { url: String, message: String },

    #[error("git ref-advertisement response from {url} was malformed: {message}")]
    MalformedRefs { url: String, message: String },

    #[error("{url} has no ref named {requested_ref:?}")]
    RefNotFound { url: String, requested_ref: String },

    #[error("{url} advertises no HEAD ref")]
    NoHead { url: String },

    #[error(transparent)]
    Archive(#[from] crate::registry::Error),

    #[error("git codeload cache error for {path}: {message}")]
    Cache { path: String, message: String },
}

fn cache_copy_error(path: &Utf8Path, source: crate::Error) -> Error {
    Error::Cache {
        path: path.to_string(),
        message: source.to_string(),
    }
}

/// The resolved ref advertisement for one git remote: every `refs/heads/*`
/// and `refs/tags/*` entry (tags peeled to their commit sha, never the tag
/// object sha) plus HEAD's sha for unpinned adds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitRefs {
    pub head_sha: Option<String>,
    /// Ref name (`refs/heads/main`, `refs/tags/v1`) to commit sha.
    pub refs: BTreeMap<String, String>,
}

impl GitRefs {
    /// Resolve `requested_ref` (a short branch/tag name, a full 40-hex commit
    /// sha, or `None` for HEAD) to a commit sha. Short names are tried
    /// against `refs/heads/<name>` then `refs/tags/<name>` before the
    /// literal string, so callers never have to know which kind of ref they
    /// were given. A raw 40-hex sha is never a key in the advertised
    /// ref-name map (it names a commit, not a ref), so it is returned
    /// directly without a map lookup — this is what makes a locked/pinned
    /// commit (e.g. from `--all`'s `git+...#ref=<sha>` specs) resolvable at
    /// all.
    pub fn resolve<'a>(&'a self, requested_ref: Option<&'a str>) -> Option<&'a str> {
        match requested_ref {
            None => self.head_sha.as_deref(),
            Some(name) if is_full_sha(name) => Some(name),
            Some(name) => self
                .refs
                .get(&format!("refs/heads/{name}"))
                .or_else(|| self.refs.get(&format!("refs/tags/{name}")))
                .or_else(|| self.refs.get(name))
                .map(String::as_str),
        }
    }
}

/// True when `s` is a full 40-character lowercase-or-any-case hex commit sha
/// (the shape `fetch_codeload_snapshot` and codeload URLs require).
fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn user_agent() -> String {
    format!("ctx-traits/{}", env!("CARGO_PKG_VERSION"))
}

fn request_error(url: &str, source: &ureq::Error) -> Error {
    Error::Request {
        url: url.to_string(),
        message: source.to_string(),
    }
}

/// Default git remote host the shorthand and refs resolution target.
pub const DEFAULT_REMOTE_BASE: &str = "https://github.com";

/// Resolve the effective git remote base URL for smart-HTTP ref resolution:
/// `CTX_TRAITS_GIT_REMOTE_BASE` when set and non-empty, else
/// [`DEFAULT_REMOTE_BASE`]. The test-fixture override seam for the
/// `info/refs` endpoint, mirroring `CTX_TRAITS_GIT_CODELOAD_BASE` for
/// tarballs — without it a github.com spec would hit the real host from
/// inside a proof.
pub fn resolve_remote_base() -> String {
    if let Ok(base) = std::env::var("CTX_TRAITS_GIT_REMOTE_BASE")
        && !base.is_empty()
    {
        return base;
    }
    DEFAULT_REMOTE_BASE.to_string()
}

/// Resolve the effective GitHub codeload base URL: `CTX_TRAITS_GIT_CODELOAD_BASE`
/// when set and non-empty, else [`DEFAULT_CODELOAD_BASE`].
pub fn resolve_codeload_base() -> String {
    if let Ok(base) = std::env::var("CTX_TRAITS_GIT_CODELOAD_BASE")
        && !base.is_empty()
    {
        return base;
    }
    DEFAULT_CODELOAD_BASE.to_string()
}

/// Fetch and parse the smart-HTTP ref advertisement for `url` (a git remote
/// URL, e.g. `https://github.com/owner/repo.git` or, under
/// `CTX_TRAITS_GIT_CODELOAD_BASE`, a fixture URL of the same shape).
pub fn resolve_refs(url: &str) -> Result<GitRefs, Error> {
    let request_url = format!(
        "{}/info/refs?service=git-upload-pack",
        url.trim_end_matches('/')
    );
    let response = ureq::get(&request_url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|source| request_error(&request_url, &source))?;
    if !response.status().is_success() {
        return Err(Error::Request {
            url: request_url.clone(),
            message: format!("HTTP {}", response.status()),
        });
    }
    let body = response
        .into_body()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_vec()
        .map_err(|source| request_error(&request_url, &source))?;
    parse_ref_advertisement(&request_url, &body)
}

/// Parse a `git-upload-pack` smart-HTTP ref advertisement (protocol v0
/// pkt-line stream: a `# service=...` line, a flush, then one pkt-line per
/// ref — the first ref line carries capabilities after a NUL byte). Tags
/// with a following `<sha> <ref>^{}` peeled entry are recorded under their
/// bare ref name with the peeled (dereferenced commit) sha, since that is
/// the sha a checkout of that tag actually wants.
fn parse_ref_advertisement(url: &str, body: &[u8]) -> Result<GitRefs, Error> {
    let lines = split_pkt_lines(body);
    let mut refs = BTreeMap::new();
    let mut head_sha = None;
    let mut first_ref_seen = false;

    for line in lines {
        let line = line.trim_end_matches(['\n']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = if !first_ref_seen {
            first_ref_seen = true;
            // First ref line: `<sha> <ref>\0<capabilities>`.
            line.split('\0').next().unwrap_or(line)
        } else {
            line
        };
        let Some((sha, name)) = line.split_once(' ') else {
            continue;
        };
        let sha = sha.trim();
        let name = name.trim();
        if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        if name == "HEAD" {
            head_sha = Some(sha.to_string());
            continue;
        }
        if let Some(peeled_of) = name.strip_suffix("^{}") {
            // Overwrite the tag object sha recorded for the un-peeled name
            // with the commit sha it actually points at.
            refs.insert(peeled_of.to_string(), sha.to_string());
            continue;
        }
        refs.entry(name.to_string())
            .or_insert_with(|| sha.to_string());
    }

    if refs.is_empty() && head_sha.is_none() {
        return Err(Error::MalformedRefs {
            url: url.to_string(),
            message: "no refs advertised".to_string(),
        });
    }
    Ok(GitRefs { head_sha, refs })
}

/// Split a pkt-line stream into its payload lines, stopping at each
/// flush-pkt (`0000`) and skipping the 4-hex-digit length header. Malformed
/// or truncated length headers end the stream at that point rather than
/// erroring — a defensive stance appropriate for what is, after parsing,
/// discarded on any downstream mismatch (no ref resolves, `resolve` returns
/// `None`, the caller reports a clear "ref not found").
fn split_pkt_lines(body: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    while offset + 4 <= body.len() {
        let header = &body[offset..offset + 4];
        let Ok(header_str) = std::str::from_utf8(header) else {
            break;
        };
        let Ok(length) = usize::from_str_radix(header_str, 16) else {
            break;
        };
        if length == 0 {
            // Flush-pkt: a section boundary (one precedes the ref list,
            // after the `# service=...` line; one terminates it). Skip
            // rather than stop, since a real advertisement has refs on
            // both sides of the first one.
            offset += 4;
            continue;
        }
        if length < 4 || offset + length > body.len() {
            break;
        }
        let payload = &body[offset + 4..offset + length];
        lines.push(String::from_utf8_lossy(payload).into_owned());
        offset += length;
    }
    lines
}

/// Directory root for the by-sha codeload snapshot cache, overridable via
/// `CTX_TRAITS_GIT_CACHE_ROOT` (test isolation, mirrors
/// `CTX_TRAITS_GIT_CODELOAD_BASE`'s override precedence). Sits alongside,
/// but independent of, the npm registry's own extraction cache
/// ([`crate::cache`]) since a codeload snapshot is keyed by commit sha, not
/// package/version.
fn codeload_cache_root() -> Utf8PathBuf {
    if let Ok(root) = std::env::var("CTX_TRAITS_GIT_CACHE_ROOT")
        && !root.is_empty()
    {
        return Utf8PathBuf::from(root);
    }
    Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .unwrap_or_else(|_| Utf8PathBuf::from("/tmp"))
        .join("ctx-traits-git-codeload-cache")
}

fn codeload_cache_key(owner: &str, repo: &str, sha: &str) -> String {
    format!("{owner}--{repo}--{sha}")
}

fn codeload_cache_marker(cache_dir: &Utf8Path) -> Utf8PathBuf {
    cache_dir.join(".ctx-git-cache-complete")
}

/// Fetch a GitHub codeload tarball snapshot for `owner/repo` at exactly
/// `sha` (never a ref name — a ref can move between resolution and fetch)
/// and extract it into `dest`, stripping codeload's `<repo>-<sha>/`
/// top-level prefix. Reuses the same hardened extraction walk the npm
/// registry client uses, so both content sources carry the same
/// traversal/symlink/entry-type/size guards.
///
/// A commit sha names an immutable tree, so extracted snapshots are cached
/// by `owner/repo/sha` under [`codeload_cache_root`]: a repeat fetch of a
/// sha already served (a re-run of `add`, a bare-collection listing
/// followed by an install, `--all` visiting the same collection twice)
/// never issues another codeload HTTP request — it copies the cached
/// extraction into `dest` via the crate's one safe tree-copy primitive
/// ([`crate::publish::copy_safe`]).
pub fn fetch_codeload_snapshot(
    owner: &str,
    repo: &str,
    sha: &str,
    dest: &Utf8Path,
) -> Result<(), Error> {
    let cache_dir = codeload_cache_root().join(codeload_cache_key(owner, repo, sha));
    let marker = codeload_cache_marker(&cache_dir);
    if cache_dir.is_dir() && marker.is_file() {
        return crate::publish::copy_safe(&cache_dir, dest, &[])
            .map_err(|source| cache_copy_error(&cache_dir, source));
    }

    let base = resolve_codeload_base();
    let url = format!("{base}/{owner}/{repo}/tar.gz/{sha}");
    let response = ureq::get(&url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|source| request_error(&url, &source))?;
    if !response.status().is_success() {
        return Err(Error::Request {
            url: url.clone(),
            message: format!("HTTP {}", response.status()),
        });
    }
    let bytes = response
        .into_body()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .map_err(|source| request_error(&url, &source))?;
    let context = format!("{owner}/{repo}");
    let strip_prefix = format!("{repo}-{sha}/");

    // Extract straight into the cache dir, then populate the marker and
    // copy from there — so a fetch that fails partway through never leaves
    // a marker-less cache entry mistaken for a complete one, and `dest`
    // always receives a full copy regardless of whether this call populated
    // the cache or a prior call did.
    let _ = std::fs::remove_dir_all(&cache_dir);
    crate::registry::extract_verified_tarball_with_prefix(
        &context,
        sha,
        &bytes,
        &cache_dir,
        &strip_prefix,
    )?;
    std::fs::write(&marker, b"").map_err(|source| Error::Request {
        url: url.clone(),
        message: format!("failed to write cache marker {marker}: {source}"),
    })?;

    crate::publish::copy_safe(&cache_dir, dest, &[])
        .map_err(|source| cache_copy_error(&cache_dir, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt_line(payload: &str) -> String {
        format!("{:04x}{}", payload.len() + 4, payload)
    }

    #[test]
    fn parses_head_and_branch_and_peeled_tag() {
        let mut body = String::new();
        body.push_str(&pkt_line("# service=git-upload-pack\n"));
        body.push_str("0000");
        body.push_str(&pkt_line(&format!(
            "{} HEAD\0multi_ack thin-pack\n",
            "a".repeat(40)
        )));
        body.push_str(&pkt_line(&format!("{} refs/heads/main\n", "a".repeat(40))));
        body.push_str(&pkt_line(&format!("{} refs/tags/v1\n", "b".repeat(40))));
        body.push_str(&pkt_line(&format!(
            "{} refs/tags/v1^{{}}\n",
            "c".repeat(40)
        )));
        body.push_str("0000");

        let refs = parse_ref_advertisement("https://example.test/repo.git", body.as_bytes())
            .expect("parses");
        assert_eq!(refs.head_sha.as_deref(), Some("a".repeat(40).as_str()));
        assert_eq!(
            refs.refs.get("refs/heads/main").map(String::as_str),
            Some("a".repeat(40).as_str())
        );
        // Peeled entry wins over the tag-object sha for the same ref name.
        assert_eq!(
            refs.refs.get("refs/tags/v1").map(String::as_str),
            Some("c".repeat(40).as_str())
        );
    }

    #[test]
    fn resolve_prefers_heads_then_tags_then_literal() {
        let mut refs = GitRefs {
            head_sha: Some("h".repeat(40)),
            ..GitRefs::default()
        };
        refs.refs
            .insert("refs/heads/feature".to_string(), "f".repeat(40));
        refs.refs
            .insert("refs/tags/feature".to_string(), "t".repeat(40));

        assert_eq!(refs.resolve(None), Some("h".repeat(40).as_str()));
        assert_eq!(refs.resolve(Some("feature")), Some("f".repeat(40).as_str()));
        assert_eq!(refs.resolve(Some("missing")), None);
    }

    #[test]
    fn resolve_accepts_a_raw_full_sha_absent_from_advertised_refs() {
        let mut refs = GitRefs {
            head_sha: Some("h".repeat(40)),
            ..GitRefs::default()
        };
        refs.refs
            .insert("refs/heads/main".to_string(), "m".repeat(40));

        let pinned = "d".repeat(40);
        assert_eq!(refs.resolve(Some(&pinned)), Some(pinned.as_str()));
    }

    #[test]
    fn resolve_treats_a_short_hex_like_string_as_a_ref_name_not_a_sha() {
        let refs = GitRefs::default();
        // Not 40 chars: must not be short-circuited as a sha, so it falls
        // through to the (missing) ref-name lookup.
        assert_eq!(refs.resolve(Some("deadbeef")), None);
    }

    #[test]
    fn empty_advertisement_is_malformed() {
        let mut body = String::new();
        body.push_str(&pkt_line("# service=git-upload-pack\n"));
        body.push_str("0000");
        body.push_str("0000");
        let result = parse_ref_advertisement("https://example.test/repo.git", body.as_bytes());
        assert!(matches!(result, Err(Error::MalformedRefs { .. })));
    }

    #[test]
    fn resolve_codeload_base_honors_env_override() {
        temp_env(
            "CTX_TRAITS_GIT_CODELOAD_BASE",
            Some("http://127.0.0.1:9"),
            || {
                assert_eq!(resolve_codeload_base(), "http://127.0.0.1:9");
            },
        );
        temp_env("CTX_TRAITS_GIT_CODELOAD_BASE", None, || {
            assert_eq!(resolve_codeload_base(), DEFAULT_CODELOAD_BASE);
        });
    }

    #[test]
    fn fetch_codeload_snapshot_skips_http_when_the_sha_is_already_cached() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let unique = format!(
            "ctx-traits-git-fetch-test-{}",
            std::process::id().wrapping_add(line!())
        );
        let cache_root = std::env::temp_dir().join(&unique);
        let cache_root = Utf8PathBuf::from_path_buf(cache_root).expect("utf8 path");
        let owner = "octo-owner";
        let repo = "octo-repo";
        let sha = "e".repeat(40);

        // Pre-populate the cache directly, bypassing any HTTP fetch, then
        // point the codeload base at a port nothing listens on — a real
        // fetch attempt would fail immediately with a connection error.
        let cache_dir = cache_root.join(codeload_cache_key(owner, repo, sha.as_str()));
        std::fs::create_dir_all(cache_dir.join("nested")).expect("create cache dir");
        std::fs::write(cache_dir.join("nested").join("file.txt"), b"cached content")
            .expect("write cache fixture file");
        std::fs::write(codeload_cache_marker(&cache_dir), b"").expect("write cache marker");

        let previous_cache_root = std::env::var("CTX_TRAITS_GIT_CACHE_ROOT").ok();
        let previous_codeload_base = std::env::var("CTX_TRAITS_GIT_CODELOAD_BASE").ok();
        unsafe {
            std::env::set_var("CTX_TRAITS_GIT_CACHE_ROOT", cache_root.as_str());
            std::env::set_var("CTX_TRAITS_GIT_CODELOAD_BASE", "http://127.0.0.1:1");
        }

        let dest_root = std::env::temp_dir().join(format!("{unique}-dest"));
        let dest_root = Utf8PathBuf::from_path_buf(dest_root).expect("utf8 path");
        let result = fetch_codeload_snapshot(owner, repo, &sha, &dest_root);

        unsafe {
            match previous_cache_root {
                Some(v) => std::env::set_var("CTX_TRAITS_GIT_CACHE_ROOT", v),
                None => std::env::remove_var("CTX_TRAITS_GIT_CACHE_ROOT"),
            }
            match previous_codeload_base {
                Some(v) => std::env::set_var("CTX_TRAITS_GIT_CODELOAD_BASE", v),
                None => std::env::remove_var("CTX_TRAITS_GIT_CODELOAD_BASE"),
            }
        }
        let _ = std::fs::remove_dir_all(&cache_root);
        let _ = std::fs::remove_dir_all(&dest_root);

        result.expect("cached fetch succeeds without contacting the (unreachable) codeload base");
    }

    /// ONE lock for every env-mutating test in this module: a fn-local
    /// static in one test and temp_env's own lock in another serialized
    /// nothing against each other — the exact race the run's reviewer
    /// flagged.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_env(key: &str, value: Option<&str>, body: impl FnOnce()) {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var(key).ok();
        match value {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
        body();
        match previous {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}
