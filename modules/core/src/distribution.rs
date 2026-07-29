//! Pure npm-transport trait distribution model (P438).
//!
//! Parses project-install package specs, selects versions from registry
//! metadata (semver ranges and dist-tags), models the optional `ctx.digests`
//! publisher claim, and projects a discovered trait's command/resource/agent
//! capability surface. Nothing here performs network, filesystem, or archive
//! IO — that lives in `ctx-traits-io`'s `registry`/`distribution` modules.

use std::collections::BTreeMap;

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
