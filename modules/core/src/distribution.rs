//! Pure npm-transport trait distribution model (P438).
//!
//! Parses project-install package specs, selects versions from registry
//! metadata (semver ranges and dist-tags), models the optional `ctx.digests`
//! publisher claim, and projects a discovered trait's command/resource/agent
//! capability surface. Nothing here performs network, filesystem, or archive
//! IO — that lives in `ctx-traits-io`'s `registry`/`distribution` modules.

use std::collections::BTreeMap;

use camino::{Utf8Component, Utf8Path};
use schemars::JsonSchema;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// Distribution parsing/resolution errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("empty package spec")]
    EmptySpec,

    #[error("{form} sources are not yet supported on the project install surface: {spec:?}")]
    NotYetSupported { form: &'static str, spec: String },

    #[error("invalid package spec {spec:?}: {message}")]
    InvalidSpec { spec: String, message: String },

    #[error("invalid semver range {range:?} for package {package}: {message}")]
    InvalidRange {
        package: String,
        range: String,
        message: String,
    },

    #[error("no version of {package} satisfies {requirement}")]
    NoMatchingVersion {
        package: String,
        requirement: String,
    },

    #[error("unknown dist-tag {tag:?} for package {package}")]
    UnknownDistTag { package: String, tag: String },

    #[error(
        "publisher digest claim mismatch for {trait_path}: claimed {claimed}, computed {computed}"
    )]
    ClaimMismatch {
        trait_path: String,
        claimed: String,
        computed: String,
    },
}

/// A parsed npm package identity: optional scope plus bare name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackageName {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub name: String,
}

impl PackageName {
    /// The full npm identifier, e.g. `@scope/name` or `name`.
    pub fn full(&self) -> String {
        match &self.scope {
            Some(scope) => format!("@{scope}/{name}", name = self.name),
            None => self.name.clone(),
        }
    }

    /// Default vendor alias: the unscoped basename.
    pub fn default_alias(&self) -> &str {
        &self.name
    }
}

/// A version selector requested at the CLI edge or recorded in the project
/// manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum VersionSelector {
    /// No selector given: resolve the highest stable version.
    Latest,
    /// An npm dist-tag (e.g. `next`, `latest`).
    DistTag(String),
    /// An explicit semver range (e.g. `^1.2.0`, `1.2.3`, `>=1 <2`).
    Range(String),
}

impl VersionSelector {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Latest => "*",
            Self::DistTag(tag) => tag,
            Self::Range(range) => range,
        }
    }
}

/// A parsed project-install package spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct PackageSpec {
    pub package: PackageName,
    pub selector: VersionSelector,
    /// Default vendor alias derived from the package name. Callers may
    /// override this when the alias collides with an existing dependency.
    pub default_alias: String,
}

/// Parse a project-install package spec.
///
/// Accepts `name`, `name@range`, `@scope/name`, `@scope/name@range`, and
/// dist-tags such as `name@next`. Explicitly rejects `git+...` and `path:...`
/// forms with [`Error::NotYetSupported`] rather than silently misparsing them
/// as npm package names.
pub fn parse_spec(input: &str) -> Result<PackageSpec, Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::EmptySpec);
    }
    if trimmed.starts_with("git+") || trimmed.starts_with("git://") {
        return Err(Error::NotYetSupported {
            form: "git",
            spec: trimmed.to_string(),
        });
    }
    if trimmed.starts_with("path:") {
        return Err(Error::NotYetSupported {
            form: "path",
            spec: trimmed.to_string(),
        });
    }

    let (name_part, selector_part) = if let Some(rest) = trimmed.strip_prefix('@') {
        // Scoped package: the scope's leading `@` is not a version separator;
        // look for the *next* `@` (the version separator) after the scope.
        match rest.find('@') {
            Some(at) => (&trimmed[..at + 1], Some(&rest[at + 1..])),
            None => (trimmed, None),
        }
    } else {
        match trimmed.find('@') {
            Some(at) => (&trimmed[..at], Some(&trimmed[at + 1..])),
            None => (trimmed, None),
        }
    };

    let package = parse_package_name(name_part, trimmed)?;

    let selector = match selector_part {
        None | Some("") | Some("latest") => VersionSelector::Latest,
        Some(value) if looks_like_dist_tag(value) => VersionSelector::DistTag(value.to_string()),
        Some(value) => {
            // Validate the range parses as semver up front so authoring
            // mistakes fail at parse time, not deep inside resolution.
            VersionReq::parse(value).map_err(|source| Error::InvalidRange {
                package: package.full(),
                range: value.to_string(),
                message: source.to_string(),
            })?;
            VersionSelector::Range(value.to_string())
        }
    };

    let default_alias = package.default_alias().to_string();
    Ok(PackageSpec {
        package,
        selector,
        default_alias,
    })
}

/// A parsed project-scoped local `path:<relative-path>` install spec (P535).
///
/// `relative_path` is the normalized authored relative path (redundant `.`
/// components dropped, `..` components preserved verbatim so a sibling
/// repository can be named): never an absolute path, never resolved against
/// any filesystem root. Resolution against a consuming project's root, and
/// every filesystem safety check, happens at the IO boundary
/// ([`ctx_traits_io::distribution`]) — this type only carries the parsed,
/// committed-manifest-safe text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PathSpec {
    pub relative_path: String,
    /// Default vendor alias derived from the path's final named component.
    pub default_alias: String,
}

/// A parsed git-transport install spec (task 0191, phase a). Normalizes every
/// authoring form (shorthand, full URL, `git+` explicit) to the same shape
/// consumed by [`crate::manifest::source::TraitSource::Git`] at the IO
/// boundary: a URL, an optional pinned ref, and an optional trait selector
/// within the fetched collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitSpec {
    /// Full Git repository URL (never GitHub shorthand — that is expanded to
    /// this field's value by the parser below).
    pub url: String,
    /// Requested tag, branch, or revision. `None` resolves the remote
    /// default branch tip at IO time.
    pub requested_ref: Option<String>,
    /// The trait name/path selected within the collection. `None` means the
    /// spec named only a collection (bare `owner/repo`), which the IO layer
    /// resolves by listing rather than vendoring.
    pub trait_selector: Option<String>,
    /// Default vendor alias: the selector's final path component, or the
    /// repo's final path component when no selector was given.
    pub default_alias: String,
}

/// The union of transports a project-scoped install spec may name: an npm
/// registry package, a local `path:` source (P535), or a git-transport
/// source (task 0191). Distinct from [`PackageSpec`]/[`parse_spec`], which
/// remain npm-only and byte-compatible with every pre-P535 caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", tag = "transport")]
pub enum InstallSpec {
    Npm(PackageSpec),
    Path(PathSpec),
    Git(GitSpec),
}

/// Parse a project-scoped install spec: `path:<relative-path>` (P535), a git
/// shorthand/URL form (task 0191), or any npm spec `parse_spec` accepts. The
/// one entry point that recognizes `path:` and git forms; `parse_spec` itself
/// keeps refusing them with [`Error::NotYetSupported`] for every caller that
/// has not opted into these transports.
pub fn parse_install_spec(input: &str) -> Result<InstallSpec, Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::EmptySpec);
    }
    if let Some(rest) = trimmed.strip_prefix("path:") {
        return parse_path_spec(rest, trimmed).map(InstallSpec::Path);
    }
    if let Some(spec) = try_parse_git_spec(trimmed)? {
        return Ok(InstallSpec::Git(spec));
    }
    parse_spec(trimmed).map(InstallSpec::Npm)
}

/// Recognize and parse the git-transport forms (task 0191, phase a):
/// `owner/repo[/trait][@ref]` shorthand, full `https://`/`git@host:` URLs
/// (with an optional bare `@ref` suffix — only meaningful on the shorthand
/// form; explicit URLs pin via `--trait`/`#ref=` at the caller), and the
/// explicit `git+<url>#ref=...&path=...` round-trip form. Returns `Ok(None)`
/// for input that names none of these forms (npm bare/scoped names), so the
/// caller falls through to `parse_spec`.
///
/// A leading `@` with exactly three shorthand segments (`@scope/catalog/x`)
/// is the phase-(d) nested npm catalog form: rejected here with a message
/// naming that phase rather than falling through to a misleading
/// invalid-name-segment error from `parse_spec`.
fn try_parse_git_spec(trimmed: &str) -> Result<Option<GitSpec>, Error> {
    if let Some(rest) = trimmed.strip_prefix("git+") {
        return parse_explicit_git_url_spec(rest, trimmed).map(Some);
    }
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return parse_bare_git_url_spec(trimmed).map(Some);
    }
    if trimmed.starts_with("git@") && trimmed.contains(':') {
        return parse_bare_git_url_spec(trimmed).map(Some);
    }
    if trimmed.starts_with('@') {
        let segments: Vec<&str> = trimmed.split('/').collect();
        if segments.len() == 3 {
            return Err(Error::NotYetSupported {
                form: "npm-nested (phase d: nested npm catalog)",
                spec: trimmed.to_string(),
            });
        }
        return Ok(None);
    }
    if trimmed.contains('/') {
        return parse_shorthand_git_spec(trimmed).map(Some);
    }
    Ok(None)
}

/// `owner/repo[/trait][@ref]`: 2 segments name a bare collection (no
/// selector — the IO layer lists it), 3 segments select a trait *name*
/// within it (resolved by probe order at IO time, never treated as a literal
/// path here).
fn parse_shorthand_git_spec(trimmed: &str) -> Result<GitSpec, Error> {
    let (path_part, ref_part) = split_trailing_ref(trimmed);
    let segments: Vec<&str> = path_part.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(Error::InvalidSpec {
            spec: trimmed.to_string(),
            message: "git shorthand must be owner/repo or owner/repo/trait, with no empty segments"
                .to_string(),
        });
    }
    match segments.as_slice() {
        [owner, repo] => {
            let url = format!("https://github.com/{owner}/{repo}");
            Ok(GitSpec {
                default_alias: (*repo).to_string(),
                url,
                requested_ref: ref_part.map(str::to_string),
                trait_selector: None,
            })
        }
        [owner, repo, trait_name] => {
            let url = format!("https://github.com/{owner}/{repo}");
            Ok(GitSpec {
                default_alias: (*trait_name).to_string(),
                url,
                requested_ref: ref_part.map(str::to_string),
                trait_selector: Some((*trait_name).to_string()),
            })
        }
        _ => Err(Error::InvalidSpec {
            spec: trimmed.to_string(),
            message: "git shorthand must be owner/repo or owner/repo/trait".to_string(),
        }),
    }
}

/// A full `https://`/`git@host:` URL with an optional trailing `@ref`
/// (mirroring the shorthand's pin syntax). No trait selector: the caller
/// supplies one via `--trait` at the CLI edge, or the spec names a bare
/// collection to list.
fn parse_bare_git_url_spec(trimmed: &str) -> Result<GitSpec, Error> {
    let (url_part, ref_part) = split_trailing_ref(trimmed);
    if url_part.is_empty() {
        return Err(Error::InvalidSpec {
            spec: trimmed.to_string(),
            message: "empty git URL".to_string(),
        });
    }
    let default_alias = url_repo_basename(url_part).unwrap_or_else(|| "repo".to_string());
    Ok(GitSpec {
        url: url_part.to_string(),
        requested_ref: ref_part.map(str::to_string),
        trait_selector: None,
        default_alias,
    })
}

/// `git+<url>#ref=<ref>&path=<package_path>`: the canonical round-trip form
/// written back by `spec_input()` / read back from `config.toml`. `path`
/// here is the trait's package path within the repo, carried straight
/// through as `trait_selector` (already resolved, not probed).
fn parse_explicit_git_url_spec(rest: &str, full_spec: &str) -> Result<GitSpec, Error> {
    let (url_part, fragment) = match rest.split_once('#') {
        Some((url, fragment)) => (url, Some(fragment)),
        None => (rest, None),
    };
    if url_part.is_empty() {
        return Err(Error::InvalidSpec {
            spec: full_spec.to_string(),
            message: "empty git URL".to_string(),
        });
    }
    let mut requested_ref = None;
    let mut trait_selector = None;
    if let Some(fragment) = fragment {
        for pair in fragment.split('&') {
            if pair.is_empty() {
                continue;
            }
            match pair.split_once('=') {
                Some(("ref", value)) if !value.is_empty() => {
                    requested_ref = Some(value.to_string());
                }
                Some(("path", value)) if !value.is_empty() => {
                    trait_selector = Some(value.to_string());
                }
                _ => {
                    return Err(Error::InvalidSpec {
                        spec: full_spec.to_string(),
                        message: format!("unrecognized git+ fragment segment {pair:?}"),
                    });
                }
            }
        }
    }
    let default_alias = trait_selector
        .as_deref()
        .and_then(|selector| selector.rsplit('/').find(|part| !part.is_empty()))
        .map(str::to_string)
        .or_else(|| url_repo_basename(url_part))
        .unwrap_or_else(|| "repo".to_string());
    Ok(GitSpec {
        url: url_part.to_string(),
        requested_ref,
        trait_selector,
        default_alias,
    })
}

/// Split a trailing `@<ref>` pin suffix from a shorthand or bare-URL spec.
/// Only the *last* `@` counts, and only when it comes after the last `/` (so
/// `git@github.com:owner/repo` is not misread as a ref pin on the host part)
/// and after any `://` scheme separator.
fn split_trailing_ref(spec: &str) -> (&str, Option<&str>) {
    let scheme_end = spec.find("://").map(|idx| idx + 3).unwrap_or(0);
    let search_from = &spec[scheme_end..];
    let last_slash = search_from.rfind('/');
    let Some(at) = search_from.rfind('@') else {
        return (spec, None);
    };
    if let Some(slash) = last_slash {
        if at < slash {
            return (spec, None);
        }
    } else if scheme_end == 0 {
        // No scheme and no slash after the search window means this is a
        // bare `git@host:` SSH form's own `@`, not a ref pin — never split.
        return (spec, None);
    }
    let absolute_at = scheme_end + at;
    let ref_value = &spec[absolute_at + 1..];
    if ref_value.is_empty() {
        return (spec, None);
    }
    (&spec[..absolute_at], Some(ref_value))
}

/// Best-effort repo basename for a default alias: the last non-empty
/// `/`-separated segment, with a trailing `.git` stripped.
fn url_repo_basename(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit('/').find(|part| !part.is_empty())?;
    let without_git = last.strip_suffix(".git").unwrap_or(last);
    if without_git.is_empty() {
        None
    } else {
        Some(without_git.to_string())
    }
}

/// Validate and normalize a manifest-authored `path` source value (P535),
/// applying exactly the same rules as `path:<relative-path>` install specs:
/// non-empty, relative, `.` dropped, `..` preserved. Shared by the project
/// manifest's hand-written decoder so an empty or absolute `path` value
/// cannot enter a committed manifest through a route that bypasses
/// [`parse_install_spec`].
pub fn normalize_manifest_path_source(raw: &str) -> Result<String, Error> {
    parse_path_spec(raw, raw).map(|spec| spec.relative_path)
}

/// True when `rest` carries a machine-specific absolute or drive-qualified
/// identity in ANY host's path syntax, not just the syntax of the platform
/// this binary happens to run on. `camino::Utf8Path::is_absolute` and
/// `Utf8Component` only recognize the current host's separators and prefix
/// forms — on Unix, a Windows drive-qualified path (`C:\Users\name\pkg`),
/// drive-relative path (`C:pkg`), or UNC path (`\\server\share\pkg`) parses
/// as ordinary relative "normal" components and would otherwise be accepted
/// and persisted into a committed manifest/lock, giving the same committed
/// dependency line a different (and machine-specific) meaning depending on
/// which host authored it.
fn has_foreign_absolute_marker(rest: &str) -> bool {
    if rest.contains('\\') {
        return true;
    }
    let bytes = rest.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        // Windows drive-qualified (`C:\...`, `C:/...`) or drive-relative
        // (`C:foo`) form.
        return true;
    }
    false
}

fn parse_path_spec(rest: &str, full_spec: &str) -> Result<PathSpec, Error> {
    if rest.trim().is_empty() {
        return Err(Error::InvalidSpec {
            spec: full_spec.to_string(),
            message: "path: requires a non-empty relative path".to_string(),
        });
    }
    if has_foreign_absolute_marker(rest) {
        return Err(Error::InvalidSpec {
            spec: full_spec.to_string(),
            message: "path: must be a slash-separated relative path, not an absolute or drive-qualified path"
                .to_string(),
        });
    }
    let path = Utf8Path::new(rest);
    if path.is_absolute() {
        return Err(Error::InvalidSpec {
            spec: full_spec.to_string(),
            message: "path: must be a relative path, not absolute".to_string(),
        });
    }
    let mut normalized: Vec<&str> = Vec::new();
    for component in path.components() {
        match component {
            Utf8Component::Normal(part) => normalized.push(part),
            Utf8Component::ParentDir => normalized.push(".."),
            Utf8Component::CurDir => {}
            Utf8Component::RootDir | Utf8Component::Prefix(_) => {
                return Err(Error::InvalidSpec {
                    spec: full_spec.to_string(),
                    message: "path: must be a relative path, not absolute".to_string(),
                });
            }
        }
    }
    if normalized.is_empty() {
        return Err(Error::InvalidSpec {
            spec: full_spec.to_string(),
            message: "path: requires a non-empty relative path".to_string(),
        });
    }
    let Some(default_alias) = normalized.iter().rev().find(|part| **part != "..") else {
        return Err(Error::InvalidSpec {
            spec: full_spec.to_string(),
            message: "path: must name a directory, not only parent-directory segments".to_string(),
        });
    };
    Ok(PathSpec {
        relative_path: normalized.join("/"),
        default_alias: default_alias.to_string(),
    })
}

/// A selector value is treated as a dist-tag rather than a semver range when
/// it fails to parse as either a bare version or a range expression.
fn looks_like_dist_tag(value: &str) -> bool {
    Version::parse(value).is_err() && VersionReq::parse(value).is_err()
}

fn parse_package_name(name_part: &str, full_spec: &str) -> Result<PackageName, Error> {
    if name_part.is_empty() {
        return Err(Error::InvalidSpec {
            spec: full_spec.to_string(),
            message: "empty package name".to_string(),
        });
    }
    if let Some(rest) = name_part.strip_prefix('@') {
        let Some((scope, name)) = rest.split_once('/') else {
            return Err(Error::InvalidSpec {
                spec: full_spec.to_string(),
                message: "scoped package must be @scope/name".to_string(),
            });
        };
        if scope.is_empty() || name.is_empty() {
            return Err(Error::InvalidSpec {
                spec: full_spec.to_string(),
                message: "scoped package must be @scope/name".to_string(),
            });
        }
        validate_npm_name_segment(scope, full_spec)?;
        validate_npm_name_segment(name, full_spec)?;
        Ok(PackageName {
            scope: Some(scope.to_string()),
            name: name.to_string(),
        })
    } else {
        validate_npm_name_segment(name_part, full_spec)?;
        Ok(PackageName {
            scope: None,
            name: name_part.to_string(),
        })
    }
}

fn validate_npm_name_segment(segment: &str, full_spec: &str) -> Result<(), Error> {
    let valid = !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    if !valid {
        return Err(Error::InvalidSpec {
            spec: full_spec.to_string(),
            message: format!("invalid npm package name segment {segment:?}"),
        });
    }
    Ok(())
}

/// Resolve a version selector against registry metadata.
///
/// - A dist-tag selector resolves through `dist_tags`.
/// - A range selector chooses the highest version satisfying the range;
///   prereleases are only admitted when the range itself names one (the
///   `semver` crate's native matching behavior).
/// - `Latest` chooses the highest non-prerelease version.
pub fn resolve_version(
    package: &str,
    versions: &[String],
    dist_tags: &BTreeMap<String, String>,
    selector: &VersionSelector,
) -> Result<String, Error> {
    match selector {
        VersionSelector::DistTag(tag) => {
            dist_tags
                .get(tag.as_str())
                .cloned()
                .ok_or_else(|| Error::UnknownDistTag {
                    package: package.to_string(),
                    tag: tag.clone(),
                })
        }
        VersionSelector::Range(range) => {
            let requirement = VersionReq::parse(range).map_err(|source| Error::InvalidRange {
                package: package.to_string(),
                range: range.clone(),
                message: source.to_string(),
            })?;
            highest_matching(versions, |version| requirement.matches(version)).ok_or_else(|| {
                Error::NoMatchingVersion {
                    package: package.to_string(),
                    requirement: range.clone(),
                }
            })
        }
        VersionSelector::Latest => highest_matching(versions, |version| version.pre.is_empty())
            .ok_or_else(|| Error::NoMatchingVersion {
                package: package.to_string(),
                requirement: "latest stable".to_string(),
            }),
    }
}

fn highest_matching(versions: &[String], predicate: impl Fn(&Version) -> bool) -> Option<String> {
    versions
        .iter()
        .filter_map(|raw| Version::parse(raw).ok().map(|parsed| (raw, parsed)))
        .filter(|(_, parsed)| predicate(parsed))
        .max_by(|(_, left), (_, right)| left.cmp(right))
        .map(|(raw, _)| raw.clone())
}

/// A caret range anchored at a resolved stable version, written to the
/// project manifest when `install` is given no explicit selector.
pub fn caret_range(resolved_version: &str) -> String {
    format!("^{resolved_version}")
}

/// Spec string for replaying an already-locked exact version, shared by
/// every locked-entry replay path (a project package or an `extends` base).
/// A bare version like `1.0.0` parses as a *caret* requirement (the semver
/// crate's default comparator), so `package@1.0.0` matches any later
/// same-major version too — a locked replay that used that form would float
/// onto a newer compatible version the instant the registry started
/// offering one, rather than reproducing the exact locked bytes. The `=`
/// comparator selects that version and no other.
pub fn exact_version_spec(package: &str, version: &str) -> String {
    format!("{package}@={version}")
}

// ---------------------------------------------------------------------------
// Publisher claims (`ctx.digests`)
// ---------------------------------------------------------------------------

/// The optional `ctx.digests` publisher claim: a per-trait canonical-relative-
/// path-to-canonical-digest map, read from the npm package's `package.json`
/// `ctx.digests` field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PublisherDigestClaim {
    pub digests: BTreeMap<String, String>,
}

/// Deterministic description of one file in a publish payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PublishFile {
    pub path: String,
    pub digest: String,
}

/// Pure publish report shared by dry-run and real publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct PublishPlan {
    pub package: String,
    pub version: String,
    pub provenance: bool,
    pub files: Vec<PublishFile>,
    pub digests: BTreeMap<String, String>,
    /// Directories skipped by the pack exclude set, so an operator can see
    /// what was left out rather than infer it from the tarball's absence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedPath>,
}

/// One directory the pack exclude set dropped from the tarball: its
/// npm-root-relative path and the exclude rule (bare directory name) that
/// matched it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct SkippedPath {
    pub path: String,
    pub rule: String,
}

/// Outcome of verifying a publisher claim against computed canonical digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaimVerification {
    /// No claim was present in the package.
    Absent,
    /// The claim's entries agree exactly with the computed digests.
    Verified,
}

/// Verify a publisher claim against the computed canonical digests for every
/// discovered trait in a package, keyed by canonical relative path.
///
/// Requires exact agreement as a set: a claim missing an entry for a
/// discovered trait, or claiming a path that was not discovered, is also a
/// mismatch, since a partial claim could otherwise hide a swapped trait.
pub fn verify_publisher_claim(
    claim: Option<&PublisherDigestClaim>,
    computed: &BTreeMap<String, String>,
) -> Result<ClaimVerification, Error> {
    let Some(claim) = claim else {
        return Ok(ClaimVerification::Absent);
    };
    // Missing-on-either-side entries are reported as a mismatch against an
    // empty claimed/computed digest, so a partial claim can never be mistaken
    // for a verified one.
    if let Some(path) = computed
        .keys()
        .find(|path| !claim.digests.contains_key(path.as_str()))
    {
        return Err(Error::ClaimMismatch {
            trait_path: path.clone(),
            claimed: String::new(),
            computed: computed.get(path).cloned().unwrap_or_default(),
        });
    }
    for (path, claimed_digest) in &claim.digests {
        let Some(computed_digest) = computed.get(path) else {
            return Err(Error::ClaimMismatch {
                trait_path: path.clone(),
                claimed: claimed_digest.clone(),
                computed: String::new(),
            });
        };
        if computed_digest != claimed_digest {
            return Err(Error::ClaimMismatch {
                trait_path: path.clone(),
                claimed: claimed_digest.clone(),
                computed: computed_digest.clone(),
            });
        }
    }
    Ok(ClaimVerification::Verified)
}

// ---------------------------------------------------------------------------
// Capability-surface projection
// ---------------------------------------------------------------------------

/// A reported command's argv or its dynamic `argv-from` source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CommandSource {
    Argv(Vec<String>),
    ArgvFrom(String),
}

/// The command/resource/agent capability surface of one discovered trait,
/// projected without executing anything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct CapabilitySurface {
    pub commands: Vec<CommandSource>,
    pub resource_roots: Vec<String>,
    pub agent_roles: Vec<String>,
}

/// Project the capability surface of a trait: every command's argv (or its
/// `argv-from` port), every distinct resource root kind, and every declared
/// agent role. Walks both the top-level procedure body and every named
/// sequence declaration, deduplicating reports across both.
///
/// Commands are lowered through [`crate::r#trait::procedure::command_plan_for_item`],
/// the same canonical command-plan constructor validation/runtime use, so
/// reported argv can never drift from what would actually execute.
pub fn project_capability_surface(trait_ref: &crate::Trait) -> crate::Result<CapabilitySurface> {
    let mut commands = Vec::new();
    let mut resource_roots = std::collections::BTreeSet::new();
    let mut agent_roles = std::collections::BTreeSet::new();

    let mut items: Vec<(&crate::r#trait::procedure::SequenceItem, String)> = Vec::new();
    if let Some(procedure) = &trait_ref.procedure {
        for (index, item) in procedure.sequence.iter().enumerate() {
            items.push((item, format!("procedure.sequence[{index}]")));
        }
    }
    for (name, sequence) in trait_ref.sequences.iter() {
        for (index, item) in sequence.sequence.iter().enumerate() {
            items.push((item, format!("sequences.{name}.sequence[{index}]")));
        }
    }

    for (item, field_path) in items {
        if let Some(role) = &item.agent {
            agent_roles.insert(role.clone());
        }
        if item.effective_kind() != crate::r#trait::procedure::SequenceKind::Command {
            continue;
        }
        if let Some(plan) = crate::r#trait::procedure::command_plan_for_item(item, &field_path)? {
            if !plan.argv.is_empty() {
                commands.push(CommandSource::Argv(plan.argv));
            } else if let Some(argv_from) = plan.argv_from {
                commands.push(CommandSource::ArgvFrom(argv_from));
            }
        }
    }

    for agent in &trait_ref.agents {
        agent_roles.insert(agent.id.clone());
    }

    for resource in &trait_ref.resources {
        resource_roots.insert(resource_root_label(&resource.root).to_string());
    }

    Ok(CapabilitySurface {
        commands,
        resource_roots: resource_roots.into_iter().collect(),
        agent_roles: agent_roles.into_iter().collect(),
    })
}

fn resource_root_label(root: &crate::r#trait::resource::ResourceRoot) -> &'static str {
    match root {
        crate::r#trait::resource::ResourceRoot::Package => "package",
        crate::r#trait::resource::ResourceRoot::Repo => "repo",
    }
}

#[cfg(test)]
mod install_spec_tests {
    use super::*;

    #[test]
    fn npm_specs_still_reject_path_and_git_via_parse_spec() {
        assert!(matches!(
            parse_spec("path:../sibling"),
            Err(Error::NotYetSupported { form: "path", .. })
        ));
        assert!(matches!(
            parse_spec("git+https://example.com/x"),
            Err(Error::NotYetSupported { form: "git", .. })
        ));
    }

    #[test]
    fn parse_install_spec_still_parses_npm_specs_byte_compatibly() {
        let direct = parse_spec("@scope/name@^1.2.0").unwrap();
        let InstallSpec::Npm(via_union) = parse_install_spec("@scope/name@^1.2.0").unwrap() else {
            panic!("expected npm variant");
        };
        assert_eq!(direct, via_union);
    }

    #[test]
    fn parse_install_spec_decodes_a_relative_path() {
        let InstallSpec::Path(spec) =
            parse_install_spec("path:.ctx/traits/authored/implement").unwrap()
        else {
            panic!("expected path variant");
        };
        assert_eq!(spec.relative_path, ".ctx/traits/authored/implement");
        assert_eq!(spec.default_alias, "implement");
    }

    #[test]
    fn parse_install_spec_normalizes_current_dir_components() {
        let InstallSpec::Path(spec) = parse_install_spec("path:./packages/agents").unwrap() else {
            panic!("expected path variant");
        };
        assert_eq!(spec.relative_path, "packages/agents");
        assert_eq!(spec.default_alias, "agents");
    }

    #[test]
    fn parse_install_spec_preserves_parent_dir_traversal_for_sibling_repos() {
        let InstallSpec::Path(spec) =
            parse_install_spec("path:../ctx-gate/.ctx/traits/authored/refactor").unwrap()
        else {
            panic!("expected path variant");
        };
        assert_eq!(
            spec.relative_path,
            "../ctx-gate/.ctx/traits/authored/refactor"
        );
        assert_eq!(spec.default_alias, "refactor");
    }

    #[test]
    fn parse_install_spec_rejects_absolute_paths() {
        assert!(matches!(
            parse_install_spec("path:/etc/passwd"),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn parse_install_spec_rejects_empty_paths() {
        assert!(matches!(
            parse_install_spec("path:"),
            Err(Error::InvalidSpec { .. })
        ));
        assert!(matches!(
            parse_install_spec("path:   "),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn parse_install_spec_rejects_only_parent_dir_segments() {
        assert!(matches!(
            parse_install_spec("path:../.."),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn parse_install_spec_rejects_empty_input() {
        assert!(matches!(parse_install_spec(""), Err(Error::EmptySpec)));
        assert!(matches!(parse_install_spec("   "), Err(Error::EmptySpec)));
    }

    #[test]
    fn parse_install_spec_rejects_windows_absolute_paths_on_every_host() {
        assert!(matches!(
            parse_install_spec(r"path:C:\dev\sibling\package"),
            Err(Error::InvalidSpec { .. })
        ));
        assert!(matches!(
            parse_install_spec("path:C:/dev/sibling/package"),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn parse_install_spec_rejects_windows_drive_relative_paths_on_every_host() {
        assert!(matches!(
            parse_install_spec("path:C:package"),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn parse_install_spec_rejects_unc_paths_on_every_host() {
        assert!(matches!(
            parse_install_spec(r"path:\\server\share\package"),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn parse_install_spec_rejects_bare_backslash_separators() {
        assert!(matches!(
            parse_install_spec(r"path:sibling\package"),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn parse_install_spec_still_accepts_slash_separated_paths_with_parent_traversal() {
        let InstallSpec::Path(spec) =
            parse_install_spec("path:../sibling-repo/.ctx/traits/authored/implement").unwrap()
        else {
            panic!("expected path variant");
        };
        assert_eq!(
            spec.relative_path,
            "../sibling-repo/.ctx/traits/authored/implement"
        );
    }

    #[test]
    fn git_shorthand_two_segments_is_a_bare_collection() {
        let InstallSpec::Git(spec) = parse_install_spec("ctx-company/traits").unwrap() else {
            panic!("expected git variant");
        };
        assert_eq!(spec.url, "https://github.com/ctx-company/traits");
        assert_eq!(spec.requested_ref, None);
        assert_eq!(spec.trait_selector, None);
        assert_eq!(spec.default_alias, "traits");
    }

    #[test]
    fn git_shorthand_three_segments_selects_a_trait() {
        let InstallSpec::Git(spec) = parse_install_spec("ctx-company/traits/refactor").unwrap()
        else {
            panic!("expected git variant");
        };
        assert_eq!(spec.url, "https://github.com/ctx-company/traits");
        assert_eq!(spec.requested_ref, None);
        assert_eq!(spec.trait_selector.as_deref(), Some("refactor"));
        assert_eq!(spec.default_alias, "refactor");
    }

    #[test]
    fn git_shorthand_pins_a_trailing_ref() {
        let InstallSpec::Git(spec) =
            parse_install_spec("ctx-company/traits/refactor@v0.2.0").unwrap()
        else {
            panic!("expected git variant");
        };
        assert_eq!(spec.requested_ref.as_deref(), Some("v0.2.0"));
        assert_eq!(spec.trait_selector.as_deref(), Some("refactor"));
    }

    #[test]
    fn git_shorthand_rejects_empty_segments() {
        assert!(matches!(
            parse_install_spec("ctx-company//refactor"),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn git_bare_https_url_has_no_selector() {
        let InstallSpec::Git(spec) =
            parse_install_spec("https://github.com/ctx-company/traits").unwrap()
        else {
            panic!("expected git variant");
        };
        assert_eq!(spec.url, "https://github.com/ctx-company/traits");
        assert_eq!(spec.trait_selector, None);
        assert_eq!(spec.default_alias, "traits");
    }

    #[test]
    fn git_bare_https_url_pins_a_trailing_ref() {
        let InstallSpec::Git(spec) =
            parse_install_spec("https://github.com/ctx-company/traits@v0.2.0").unwrap()
        else {
            panic!("expected git variant");
        };
        assert_eq!(spec.url, "https://github.com/ctx-company/traits");
        assert_eq!(spec.requested_ref.as_deref(), Some("v0.2.0"));
    }

    #[test]
    fn git_ssh_shorthand_url_is_not_misread_as_ref_pinned() {
        let InstallSpec::Git(spec) =
            parse_install_spec("git@github.com:ctx-company/traits.git").unwrap()
        else {
            panic!("expected git variant");
        };
        assert_eq!(spec.url, "git@github.com:ctx-company/traits.git");
        assert_eq!(spec.requested_ref, None);
        assert_eq!(spec.default_alias, "traits");
    }

    #[test]
    fn git_explicit_url_round_trips_ref_and_path_fragment() {
        let InstallSpec::Git(spec) = parse_install_spec(
            "git+https://example.com/x/y#ref=v0.2.0&path=.ctx/traits/authored/refactor",
        )
        .unwrap() else {
            panic!("expected git variant");
        };
        assert_eq!(spec.url, "https://example.com/x/y");
        assert_eq!(spec.requested_ref.as_deref(), Some("v0.2.0"));
        assert_eq!(
            spec.trait_selector.as_deref(),
            Some(".ctx/traits/authored/refactor")
        );
        assert_eq!(spec.default_alias, "refactor");
    }

    #[test]
    fn git_explicit_url_without_fragment_has_no_selector() {
        let InstallSpec::Git(spec) = parse_install_spec("git+https://example.com/x/y").unwrap()
        else {
            panic!("expected git variant");
        };
        assert_eq!(spec.url, "https://example.com/x/y");
        assert_eq!(spec.requested_ref, None);
        assert_eq!(spec.trait_selector, None);
    }

    #[test]
    fn git_explicit_url_rejects_unrecognized_fragment_keys() {
        assert!(matches!(
            parse_install_spec("git+https://example.com/x/y#bogus=1"),
            Err(Error::InvalidSpec { .. })
        ));
    }

    #[test]
    fn scoped_nested_npm_catalog_form_names_phase_d_not_yet_supported() {
        let err = parse_install_spec("@scope/catalog/trait").unwrap_err();
        assert!(matches!(
            err,
            Error::NotYetSupported {
                form: "npm-nested (phase d: nested npm catalog)",
                ..
            }
        ));
    }

    #[test]
    fn scoped_npm_two_segment_names_still_parse_as_npm() {
        let InstallSpec::Npm(spec) = parse_install_spec("@scope/name@^1.2.0").unwrap() else {
            panic!("expected npm variant");
        };
        assert_eq!(spec.package.full(), "@scope/name");
    }

    #[test]
    fn unscoped_npm_names_still_parse_as_npm_not_git() {
        let InstallSpec::Npm(spec) = parse_install_spec("refactor").unwrap() else {
            panic!("expected npm variant");
        };
        assert_eq!(spec.package.full(), "refactor");
    }

    #[test]
    fn parse_spec_npm_only_stays_byte_compatible_with_git_shorthand_input() {
        // `parse_spec` (npm-only) must still refuse anything git+-prefixed;
        // bare shorthand like `owner/repo` was never valid npm input either
        // way, so this only pins the explicit-prefix contract.
        assert!(matches!(
            parse_spec("git+https://example.com/x"),
            Err(Error::NotYetSupported { form: "git", .. })
        ));
    }
}
