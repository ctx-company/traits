//! Resource evidence shared by report commands.

pub(crate) struct ResourceTextAuditEvidence {
    pub(crate) findings: Vec<ctx_traits_core::audit::Finding>,
    pub(crate) warnings: Vec<ctx_traits_io::resource::ResourceReadWarning>,
    pub(crate) skipped: Vec<String>,
}

pub(crate) fn audit_declared_text_resources(
    roots: &ctx_traits_io::resource::ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<ResourceTextAuditEvidence> {
    let mut findings = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped = Vec::new();

    for resource in &trait_ref.resources {
        // Inline content is already available to pure core; do not route it
        // through the IO scanner or manufacture host evidence for it.
        if resource.content.is_some() {
            continue;
        }
        // Repo-root resources whose effective render is `reference` are the
        // user's own repo files delivered as path references: their bytes
        // never enter the model view, so their content sits outside the
        // trait's hidden-content audit boundary. Digest/drift evidence and
        // the repo-coupled advisory still cover them.
        if repo_root_audit_excluded(resource) {
            skipped.push(format!(
                "{}:{}:{}",
                resource.id,
                resource.path.as_deref().unwrap_or(""),
                resource_text_skip_reason_label(
                    &ctx_traits_io::resource::ResourceTextSkipReason::RepoRootReference,
                )
            ));
            continue;
        }
        let outcome = ctx_traits_io::resource::read_text_resource_for_audit(roots, resource)?;
        warnings.extend(outcome.warnings.clone());
        if let Some(reason) = &outcome.skipped {
            skipped.push(format!(
                "{}:{}:{}",
                outcome.resource_id,
                outcome.path,
                resource_text_skip_reason_label(reason)
            ));
        }
        if let Some(text) = outcome.text.as_deref() {
            let path = format!("resource:{}:{}", outcome.resource_id, outcome.path);
            findings.extend(ctx_traits_core::audit::scan_hidden_content(
                text,
                trait_ref.id.as_str(),
                Some(&path),
            ));
        }
    }

    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(a.code.cmp(&b.code))
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
            .then(a.byte_offset.cmp(&b.byte_offset))
            .then(a.message.cmp(&b.message))
    });

    Ok(ResourceTextAuditEvidence {
        findings,
        warnings,
        skipped,
    })
}

fn resource_text_skip_reason_label(
    reason: &ctx_traits_io::resource::ResourceTextSkipReason,
) -> &'static str {
    match reason {
        ctx_traits_io::resource::ResourceTextSkipReason::UnsupportedExtension => {
            "unsupported-extension"
        }
        ctx_traits_io::resource::ResourceTextSkipReason::Missing => "missing",
        ctx_traits_io::resource::ResourceTextSkipReason::Symlink => "symlink",
        ctx_traits_io::resource::ResourceTextSkipReason::SpecialFile => "special-file",
        ctx_traits_io::resource::ResourceTextSkipReason::Binary => "binary",
        ctx_traits_io::resource::ResourceTextSkipReason::RepoRootReference => "repo-root-reference",
    }
}

/// Whether a resource's content is excluded from the hidden-content text
/// audit: a repo-root resource whose effective render mode never inlines it.
fn repo_root_audit_excluded(resource: &ctx_traits_core::r#trait::Resource) -> bool {
    use ctx_traits_core::r#trait::resource::ResourceRender;

    resource.effective_root() == ctx_traits_core::r#trait::ResourceRoot::Repo
        && resource.effective_render() == ResourceRender::Reference
}

/// Check warnings surfacing each resource excluded from the hidden-content
/// text audit, so the exclusion is visible evidence rather than silence.
pub(crate) fn repo_root_audit_skip_warnings(
    trait_ref: &ctx_traits_core::Trait,
) -> Vec<ctx_traits_core::check::CheckWarning> {
    trait_ref
        .resources
        .iter()
        .filter(|resource| resource.content.is_none() && repo_root_audit_excluded(resource))
        .map(|resource| ctx_traits_core::check::CheckWarning {
            section: ctx_traits_core::check::Section::Resources,
            code: "repo-root-audit-skipped".to_string(),
            field: Some(format!("resource.{}.root", resource.id)),
            message: format!(
                "resource {} is a repo-root resource with render = \"reference\": content is repo-owned and never inlined; excluded from hidden-content text audit",
                resource.id
            ),
        })
        .collect()
}

pub(crate) fn scan_resource_bodies(
    roots: &ctx_traits_io::resource::ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<(
    Vec<ctx_traits_core::resource_plan::BodyEvidence>,
    Vec<ctx_traits_io::resource::ResourceReadWarning>,
)> {
    Ok(ctx_traits_io::resource::scan_resource_bodies(
        roots, trait_ref,
    )?)
}

pub(crate) fn resource_template_candidate_ids(
    trait_ref: &ctx_traits_core::Trait,
) -> std::collections::BTreeSet<String> {
    ctx_traits_io::resource::resource_template_candidate_ids(trait_ref)
}

/// A `resource-protection` diagnostic: a declared pin whose actual bytes
/// mismatch or are unavailable, or a command/check argv resource reference
/// launched as code without a pin.
pub(crate) struct ResourceProtectionIssue {
    pub(crate) field: String,
    pub(crate) message: String,
}

/// Compare every declared resource pin against the resource manifest
/// [`build_check_report`](crate::app::report_check::build_check_report)
/// already computed, and block every command/check argv resource reference
/// whose declaration lacks a pin.
///
/// Reuses [`ctx_traits_io::resource::verify_protected_resource`] — the same
/// point-of-use verifier frame delivery and command spawn call — so `check`
/// reports exactly the protection failures runtime would independently
/// refuse, rather than a second, possibly divergent, digest comparison.
pub(crate) fn resource_protection_issues(
    roots: &ctx_traits_io::resource::ResourceRoots,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<Vec<ResourceProtectionIssue>> {
    let mut issues = Vec::new();

    for resource in &trait_ref.resources {
        if !resource.is_protected() {
            continue;
        }
        match ctx_traits_io::resource::verify_protected_resource(roots, resource)? {
            ctx_traits_io::resource::ProtectionVerification::Verified { .. } => {}
            ctx_traits_io::resource::ProtectionVerification::Unprotected => {
                unreachable!("is_protected() checked above")
            }
            ctx_traits_io::resource::ProtectionVerification::Failed(failure) => {
                issues.push(ResourceProtectionIssue {
                    field: format!("resource.{}.digest", resource.id),
                    message: failure.to_string(),
                });
            }
        }
    }

    for unpinned in ctx_traits_core::r#trait::procedure::unpinned_command_resource_argv(trait_ref) {
        issues.push(ResourceProtectionIssue {
            field: unpinned.field_path,
            message: format!(
                "command argv {{{}}} is launched as code and must be pinned with digest",
                unpinned.resource_ref
            ),
        });
    }

    issues.sort_by(|a, b| a.field.cmp(&b.field).then(a.message.cmp(&b.message)));
    Ok(issues)
}

pub(crate) fn resource_read_warning_strings(
    warnings: &[ctx_traits_io::resource::ResourceReadWarning],
) -> Vec<String> {
    use ctx_traits_io::resource::ResourceReadWarning;

    warnings
        .iter()
        .map(|warning| match warning {
            ResourceReadWarning::MissingFile { resource_id, path } => {
                format!("missing-file:{resource_id}:{path}")
            }
            ResourceReadWarning::SymlinkDetected { resource_id, path } => {
                format!("symlink-detected:{resource_id}:{path}")
            }
            ResourceReadWarning::SpecialFile { resource_id, path } => {
                format!("special-file:{resource_id}:{path}")
            }
            ResourceReadWarning::BinaryContent {
                resource_id,
                path,
                byte_size,
            } => format!("binary-content:{resource_id}:{path}:{byte_size}"),
        })
        .collect()
}
