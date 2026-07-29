use super::{Error, Marker, UnsafePathReason, control, host};
use camino::{Utf8Component, Utf8Path};
use ctx_traits_core::digest::Digest;

/// Write the leaf and every attached companion (`control::Request::with_companions`)
/// as one validated-then-written transaction: this is the sole place any
/// export/placement byte is written. Every entry — leaf and companion alike
/// — has its ancestors probed no-follow (`validate_ancestors`) and its
/// existing target, if any, checked as provably ours before any bytes are
/// written: the leaf by marker identity (`validate_leaf`), a companion by
/// recorded digest (`validate_companion_leaf`, since a companion cannot
/// carry the leaf's marker without changing the bytes its digest pins).
/// Validation runs for every entry before any write, so a refusal on one
/// entry (a symlinked ancestor, an unmanaged pre-existing file, a
/// locally-modified digest) never leaves a partially-written directory
/// behind — that guarantee covers refusals; a rarer failure inside the
/// write loop itself (a host I/O error partway through) can still leave
/// earlier entries written, exactly as the pre-existing single-leaf write
/// always could.
///
/// A caller-supplied `Fresh` companion (nothing recorded for this path —
/// `export` keeps no placement manifest, so this is its only mode) is
/// promoted to regenerable when the sibling leaf at this same target
/// already exists and is marker-owned by this exact identity: an existing
/// companion file under a leaf ctx already owns is provably ctx's own
/// prior output, not a stranger's, so re-running `export`/`host install`
/// against the same output directory is idempotent. A caller that already
/// knows better (`RecordedDigest`, `Force` from `host update`) is never
/// second-guessed by this promotion.
pub(super) fn write(
    host: &impl host::Interface,
    request: control::Request<'_>,
) -> control::Result<control::Response> {
    validate_path(request.output_root())?;
    let relative = request.relative_target();
    validate_path(&relative)?;
    let target = request.output_root().join(&relative);
    validate_path(&target)?;
    let parent = validate_target(request.output_root(), &target)?;
    validate_ancestors(host, parent)?;

    // Probe the leaf's own existing state before validating any companion:
    // a `Fresh` companion's effective ownership depends on whether the
    // leaf already proves this whole placement is ours. A leaf state that
    // would refuse the write outright (symlink, non-regular, unmanaged,
    // foreign) surfaces here rather than only after companion preflight —
    // the ultimate outcome is unchanged (`validate_leaf` runs again below
    // regardless), only the ordering of the same refusal moves earlier.
    let leaf_already_owned = validate_leaf(host, &target, request.identity())?;

    // Preflight every companion (ancestors, then ownership) before
    // touching the leaf at all: this is what makes a refusal on any single
    // entry — leaf or companion — leave nothing on disk. The leaf's own
    // marker check runs again below, after this loop and before its write,
    // as it always has (a defensive TOCTOU recheck around `create_dir_all`,
    // matching the equivalent recheck this function already does for the
    // leaf itself before and after computing the response).
    struct PlannedCompanion<'a> {
        target: camino::Utf8PathBuf,
        parent: camino::Utf8PathBuf,
        bytes: &'a [u8],
        ownership: control::CompanionOwnership<'a>,
    }
    let mut planned_companions = Vec::with_capacity(request.companions().len());
    for companion in request.companions() {
        validate_path(companion.relative_target)?;
        let companion_target = request.output_root().join(companion.relative_target);
        validate_path(&companion_target)?;
        let companion_parent =
            validate_target(request.output_root(), &companion_target)?.to_owned();
        validate_ancestors(host, &companion_parent)?;
        let effective_ownership =
            effective_companion_ownership(companion.ownership, leaf_already_owned);
        validate_companion_leaf(host, &companion_target, effective_ownership)?;
        planned_companions.push(PlannedCompanion {
            target: companion_target,
            parent: companion_parent,
            bytes: companion.bytes,
            ownership: effective_ownership,
        });
    }

    host.create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_owned(),
        source,
    })?;
    validate_ancestors(host, parent)?;
    validate_leaf(host, &target, request.identity())?;
    let bytes = request.content().as_bytes();
    let response = control::Response {
        ownership: request.identity().ownership(),
        path: target.clone(),
        content_digest: Digest::from_bytes(bytes),
        byte_size: bytes.len() as u64,
        companions: Vec::with_capacity(planned_companions.len()),
    };
    validate_ancestors(host, parent)?;
    validate_leaf(host, &target, request.identity())?;
    host.write(&target, bytes).map_err(|source| Error::Io {
        path: target,
        source,
    })?;

    let mut response = response;
    for planned in &planned_companions {
        host.create_dir_all(&planned.parent)
            .map_err(|source| Error::Io {
                path: planned.parent.clone(),
                source,
            })?;
        validate_ancestors(host, &planned.parent)?;
        validate_companion_leaf(host, &planned.target, planned.ownership)?;
        host.write(&planned.target, planned.bytes)
            .map_err(|source| Error::Io {
                path: planned.target.clone(),
                source,
            })?;
        response.companions.push(control::CompanionResponse {
            path: planned.target.clone(),
            content_digest: Digest::from_bytes(planned.bytes),
            byte_size: planned.bytes.len() as u64,
        });
    }
    Ok(response)
}
/// Promote a `Fresh` companion (the caller has no recorded digest for this
/// path) to `Force` when the sibling leaf is already marker-owned by this
/// identity — see the doc comment on [`write`]. `RecordedDigest`/`Force`
/// already reflect a caller that knows more than the leaf alone can prove,
/// so they pass through unchanged.
fn effective_companion_ownership<'a>(
    ownership: control::CompanionOwnership<'a>,
    leaf_already_owned: bool,
) -> control::CompanionOwnership<'a> {
    match ownership {
        control::CompanionOwnership::Fresh if leaf_already_owned => {
            control::CompanionOwnership::Force
        }
        other => other,
    }
}
pub(super) fn validate_target<'a>(
    output_root: &Utf8Path,
    target: &'a Utf8Path,
) -> Result<&'a Utf8Path, Error> {
    if !target.starts_with(output_root) {
        return Err(Error::EscapesOutputRoot {
            path: target.to_owned(),
        });
    }
    if inside_traits_source(target) {
        return Err(Error::InsideTraitSource {
            path: target.to_owned(),
        });
    }
    target.parent().ok_or_else(|| Error::TargetWithoutParent {
        path: target.to_owned(),
    })
}
pub(super) fn validate_path(path: &Utf8Path) -> Result<(), Error> {
    let raw = path.as_str();
    let reason = if raw.is_empty() || raw.contains('\\') || raw.contains("//") {
        Some(UnsafePathReason::UnsafeShape)
    } else if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        Some(UnsafePathReason::WindowsDrivePrefix)
    } else if path
        .components()
        .any(|c| matches!(c, Utf8Component::ParentDir))
    {
        Some(UnsafePathReason::ParentTraversal)
    } else if path
        .components()
        .any(|c| matches!(c, Utf8Component::Normal("")))
    {
        Some(UnsafePathReason::EmptySegment)
    } else {
        None
    };
    reason.map_or(Ok(()), |reason| {
        Err(Error::UnsafePath {
            path: path.to_owned(),
            reason,
        })
    })
}
fn validate_ancestors(host: &impl host::Interface, path: &Utf8Path) -> Result<(), Error> {
    let mut ancestors = Vec::new();
    let mut current = Some(path);
    while let Some(path) = current {
        ancestors.push(path);
        current = path.parent();
    }
    let mut skipped_alias = false;
    for ancestor in ancestors.into_iter().rev() {
        if path.is_absolute()
            && !skipped_alias
            && ancestor
                .parent()
                .is_some_and(|parent| parent.as_str() == "/")
        {
            skipped_alias = true;
            continue;
        }
        match host.probe(ancestor).map_err(|source| Error::Io {
            path: ancestor.to_owned(),
            source,
        })? {
            host::PathState::Missing => {}
            host::PathState::Directory => {}
            host::PathState::Symlink => {
                return Err(Error::SymlinkAncestor {
                    path: ancestor.to_owned(),
                });
            }
            _ => {
                return Err(Error::ParentNotDirectory {
                    path: ancestor.to_owned(),
                });
            }
        }
    }
    Ok(())
}
/// Validate the leaf's existing target, if any, is provably ours. Returns
/// whether it already existed and matched (`true`), or was missing
/// (`false`) — a caller preflighting companions before the leaf write uses
/// that distinction to know whether this leaf's marker already proves the
/// whole placement is ctx's own prior output.
fn validate_leaf(
    host: &impl host::Interface,
    path: &Utf8Path,
    identity: &super::Identity,
) -> Result<bool, Error> {
    match host.probe(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })? {
        host::PathState::Missing => Ok(false),
        host::PathState::Symlink => Err(Error::LeafSymlink {
            path: path.to_owned(),
        }),
        host::PathState::File => {
            let text = host.read_text(path).map_err(|source| Error::Io {
                path: path.to_owned(),
                source,
            })?;
            let marker = Marker::parse(&text).ok_or_else(|| Error::UnmanagedTarget {
                path: path.to_owned(),
            })?;
            if marker.is_owned_by(identity) {
                Ok(true)
            } else {
                Err(Error::OwnershipMismatch {
                    path: path.to_owned(),
                    existing: marker,
                })
            }
        }
        _ => Err(Error::LeafNotRegularFile {
            path: path.to_owned(),
        }),
    }
}
/// The companion counterpart to [`validate_leaf`]: a companion resource file
/// cannot carry the leaf's `> GENERATED FILE ...` marker without changing
/// the bytes its digest pins, so ownership here is digest-keyed instead —
/// see [`control::CompanionOwnership`] for what each mode means.
fn validate_companion_leaf(
    host: &impl host::Interface,
    path: &Utf8Path,
    ownership: control::CompanionOwnership<'_>,
) -> Result<(), Error> {
    match host.probe(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })? {
        host::PathState::Missing => Ok(()),
        host::PathState::Symlink => Err(Error::LeafSymlink {
            path: path.to_owned(),
        }),
        host::PathState::File => match ownership {
            control::CompanionOwnership::Force => Ok(()),
            control::CompanionOwnership::Fresh => Err(Error::UnmanagedTarget {
                path: path.to_owned(),
            }),
            control::CompanionOwnership::RecordedDigest(expected) => {
                let bytes = host.read_bytes(path).map_err(|source| Error::Io {
                    path: path.to_owned(),
                    source,
                })?;
                if &Digest::from_bytes(&bytes) == expected {
                    Ok(())
                } else {
                    Err(Error::LocallyModified {
                        path: path.to_owned(),
                    })
                }
            }
        },
        _ => Err(Error::LeafNotRegularFile {
            path: path.to_owned(),
        }),
    }
}
fn inside_traits_source(path: &Utf8Path) -> bool {
    let components: Vec<&str> = path
        .components()
        .filter_map(|component| match component {
            Utf8Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect();
    components.windows(2).enumerate().any(|(index, window)| {
        matches!(window, [".agents", "traits"])
            || matches!(window, [".ctx", "traits"]) && components.get(index + 2) != Some(&"vendor")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::host::{
        PathState,
        memory::{Operation, OperationKind, Service},
    };
    use camino::Utf8PathBuf;
    use ctx_traits_core::{
        digest::Digest,
        export::{Format, Identity, OwnershipKey},
        render::ExtendedRenderProfile,
        r#trait::Id,
    };

    fn identity(ownership: OwnershipKey) -> Identity {
        Identity::new(
            Id::new("example-trait").expect("valid ID"),
            Digest::from_bytes(b"source"),
            ownership,
        )
    }
    fn request<'a>(identity: &'a Identity, content: &'a str) -> control::Request<'a> {
        control::Request::new(Utf8Path::new("out"), identity, content, Format::Skill)
    }
    fn target() -> Utf8PathBuf {
        Utf8PathBuf::from("out/example-trait/SKILL.md")
    }

    #[test]
    fn missing_target_writes_arbitrary_content_and_records_rechecks() {
        let host = Service::new();
        let identity = identity(OwnershipKey::RenderProfile(ExtendedRenderProfile::Pi));
        let response =
            write(&host, request(&identity, "arbitrary content")).expect("write succeeds");
        assert_eq!(host.written(&target()), Some(b"arbitrary content".to_vec()));
        assert_eq!(response.path, target());
        assert_eq!(
            response.content_digest,
            Digest::from_bytes(b"arbitrary content")
        );
        assert_eq!(response.byte_size, 17);
        let operations = host.operations();
        // 3 probes of the leaf: the companion-preflight probe that decides
        // whether a `Fresh` companion is promoted to regenerable (this
        // request has no companions, but the probe still runs), plus the
        // 2 pre-existing TOCTOU rechecks around `create_dir_all`/write.
        assert!(
            operations
                .iter()
                .filter(
                    |operation| matches!(operation, Operation::Probe(path) if path == &target())
                )
                .count()
                == 3
        );
        assert!(
            matches!(operations.last(), Some(Operation::Write(path, bytes)) if path == &target() && bytes == b"arbitrary content")
        );
    }

    #[test]
    fn managed_overwrite_ignores_digest_and_replacement_marker() {
        let host = Service::new();
        let identity = identity(OwnershipKey::Skill);
        let old = Identity::new(
            identity.source_trait().clone(),
            Digest::from_bytes(b"old"),
            OwnershipKey::Skill,
        );
        host.file(target(), old.render_marker());
        write(&host, request(&identity, "replacement without marker")).expect("ownership matches");
        assert_eq!(
            host.written(&target()),
            Some(b"replacement without marker".to_vec())
        );
    }

    #[test]
    fn unmanaged_and_foreign_targets_are_refused() {
        let export_identity = identity(OwnershipKey::RenderProfile(ExtendedRenderProfile::Pi));
        let unmanaged = Service::new();
        unmanaged.file(target(), "not generated");
        assert!(matches!(
            write(&unmanaged, request(&export_identity, "new")),
            Err(Error::UnmanagedTarget { .. })
        ));
        let foreign = Service::new();
        foreign.file(
            target(),
            identity(OwnershipKey::RenderProfile(ExtendedRenderProfile::Opencode)).render_marker(),
        );
        assert!(
            matches!(write(&foreign, request(&export_identity, "new")), Err(Error::OwnershipMismatch { existing, .. }) if existing.ownership() == "opencode")
        );
    }

    #[test]
    fn agents_format_writes_agents_md_leaf() {
        let host = Service::new();
        let identity = identity(OwnershipKey::RenderProfile(ExtendedRenderProfile::Opencode));
        let response = write(
            &host,
            control::Request::new(
                Utf8Path::new("out"),
                &identity,
                "agents content",
                Format::Agents,
            ),
        )
        .expect("write succeeds");
        let expected = Utf8PathBuf::from("out/example-trait/AGENTS.md");
        assert_eq!(response.path, expected);
        assert_eq!(host.written(&expected), Some(b"agents content".to_vec()));
    }

    #[test]
    fn skill_ownership_collides_across_render_profiles() {
        let host = Service::new();
        let first = identity(OwnershipKey::Skill);
        host.file(target(), first.render_marker());
        let second = Identity::new(
            first.source_trait().clone(),
            Digest::from_bytes(b"other"),
            Format::Skill.ownership(ExtendedRenderProfile::Codex),
        );
        write(&host, request(&second, "new")).expect("skill identity is shared");
    }

    #[test]
    fn typed_refusals_and_host_errors_are_preserved() {
        let identity = identity(OwnershipKey::Skill);
        assert!(matches!(
            validate_path(Utf8Path::new("a/../b")),
            Err(Error::UnsafePath {
                reason: UnsafePathReason::ParentTraversal,
                ..
            })
        ));
        assert!(matches!(
            validate_path(Utf8Path::new("C:out")),
            Err(Error::UnsafePath {
                reason: UnsafePathReason::WindowsDrivePrefix,
                ..
            })
        ));
        assert!(matches!(
            write(
                &Service::new(),
                control::Request::new(Utf8Path::new(".ctx/traits"), &identity, "x", Format::Skill)
            ),
            Err(Error::InsideTraitSource { .. })
        ));
        let host = Service::new();
        host.state("out", PathState::Symlink);
        assert!(matches!(
            write(&host, request(&identity, "x")),
            Err(Error::SymlinkAncestor { .. })
        ));
        let host = Service::new();
        host.fail(OperationKind::Probe, std::io::ErrorKind::PermissionDenied);
        assert!(
            matches!(write(&host, request(&identity, "x")), Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn policy_refusals_cover_non_directory_ancestors_and_leaves() {
        let identity = identity(OwnershipKey::Skill);
        for state in [PathState::File, PathState::Other] {
            let host = Service::new();
            host.state("out", state);
            assert!(matches!(
                write(&host, request(&identity, "x")),
                Err(Error::ParentNotDirectory { .. })
            ));
        }
        for state in [PathState::Directory, PathState::Other] {
            let host = Service::new();
            host.state(target(), state);
            assert!(matches!(
                write(&host, request(&identity, "x")),
                Err(Error::LeafNotRegularFile { .. })
            ));
        }
    }

    #[test]
    fn defensive_target_checks_remain_live() {
        assert!(matches!(
            validate_target(Utf8Path::new("out"), Utf8Path::new("outside/SKILL.md")),
            Err(Error::EscapesOutputRoot { .. })
        ));
        assert!(matches!(
            validate_target(Utf8Path::new(""), Utf8Path::new("")),
            Err(Error::TargetWithoutParent { .. })
        ));
    }

    #[test]
    fn semantic_error_text_and_kinds_are_stable() {
        let path = Utf8PathBuf::from("out/example-trait/SKILL.md");
        let ownership_mismatch = Marker::parse(
            "> GENERATED FILE - DO NOT EDIT DIRECTLY\n> Source trait: another-trait\n> Render profile: skill",
        )
        .expect("marker parses");
        let cases = vec![
            (
                Error::UnsafePath {
                    path: path.clone(),
                    reason: UnsafePathReason::UnsafeShape,
                },
                std::io::ErrorKind::InvalidInput,
                "filesystem error at out/example-trait/SKILL.md: export path contains an unsafe path shape",
            ),
            (
                Error::UnsafePath {
                    path: path.clone(),
                    reason: UnsafePathReason::WindowsDrivePrefix,
                },
                std::io::ErrorKind::InvalidInput,
                "filesystem error at out/example-trait/SKILL.md: export path must not use a Windows drive prefix",
            ),
            (
                Error::UnsafePath {
                    path: path.clone(),
                    reason: UnsafePathReason::ParentTraversal,
                },
                std::io::ErrorKind::InvalidInput,
                "filesystem error at out/example-trait/SKILL.md: export path must not contain '..' traversal",
            ),
            (
                Error::UnsafePath {
                    path: path.clone(),
                    reason: UnsafePathReason::EmptySegment,
                },
                std::io::ErrorKind::InvalidInput,
                "filesystem error at out/example-trait/SKILL.md: export path must not contain empty path segments",
            ),
            (
                Error::EscapesOutputRoot { path: path.clone() },
                std::io::ErrorKind::PermissionDenied,
                "filesystem error at out/example-trait/SKILL.md: export target escapes output root",
            ),
            (
                Error::InsideTraitSource { path: path.clone() },
                std::io::ErrorKind::PermissionDenied,
                "filesystem error at out/example-trait/SKILL.md: refusing to write export inside trait source directory",
            ),
            (
                Error::TargetWithoutParent { path: path.clone() },
                std::io::ErrorKind::InvalidInput,
                "filesystem error at out/example-trait/SKILL.md: export target has no parent directory",
            ),
            (
                Error::SymlinkAncestor { path: path.clone() },
                std::io::ErrorKind::PermissionDenied,
                "filesystem error at out/example-trait/SKILL.md: export parent path contains a symlink; use a non-symlinked path (on macOS, the resolved /private/... path)",
            ),
            (
                Error::ParentNotDirectory { path: path.clone() },
                std::io::ErrorKind::InvalidInput,
                "filesystem error at out/example-trait/SKILL.md: export parent path is not a directory",
            ),
            (
                Error::LeafSymlink { path: path.clone() },
                std::io::ErrorKind::PermissionDenied,
                "filesystem error at out/example-trait/SKILL.md: export target leaf is a symlink",
            ),
            (
                Error::LeafNotRegularFile { path: path.clone() },
                std::io::ErrorKind::InvalidInput,
                "filesystem error at out/example-trait/SKILL.md: export target exists and is not a regular file",
            ),
            (
                Error::UnmanagedTarget { path: path.clone() },
                std::io::ErrorKind::AlreadyExists,
                "filesystem error at out/example-trait/SKILL.md: existing export target is unmanaged and will not be overwritten",
            ),
            (
                Error::OwnershipMismatch {
                    path: path.clone(),
                    existing: ownership_mismatch,
                },
                std::io::ErrorKind::AlreadyExists,
                "filesystem error at out/example-trait/SKILL.md: existing generated export belongs to a different trait or profile",
            ),
            (
                Error::Io {
                    path: path.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::TimedOut, "host timed out"),
                },
                std::io::ErrorKind::TimedOut,
                "filesystem error at out/example-trait/SKILL.md: host timed out",
            ),
        ];
        for (error, kind, text) in cases {
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), text);
        }
    }
}
