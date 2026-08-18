//! `ctx traits fork <id>` (0213): turn an installed vendored dependency at
//! `.ctx/traits/vendored/<id>` into an editable authored package at
//! `.ctx/traits/authored/<id>`.
//!
//! One transaction: copy the vendored authoring subset into a freshly
//! claimed authored root, rebuild it through the normal CDK build path
//! ([`crate::app::schema_synth_build`]), lock it against a projected
//! post-detach dependency set, record forked-from provenance, detach the
//! vendored dependency (manifest declaration + project lock entry +
//! vendored tree, via [`ctx_traits_io::distribution::remove`] — the one
//! true, self-rolling-back mutation, deliberately run last), and report
//! trust by canonical digest. The authored root is claimed atomically first
//! (the `create` precedent); any failure before `distribution::remove`
//! removes everything under it and touches nothing else, so a failed fork
//! never leaves a half-copied authored package, a false audit record, or a
//! scratch-backup leak, and the manifest/lock/vendored tree stay untouched.

use camino::Utf8Path;
use ctx_traits_core::project_lock::TraitLockEntry;
use ctx_traits_core::response::CommandOutput;
use ctx_traits_core::r#trait::TrustVerdict;
use ctx_traits_io::distribution::DistributionScope;

use crate::app::command_handlers::print_json_report;
use crate::app::lifecycle_reporting::current_utf8_dir;
use crate::app::new::trust_status_line;
use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};
use crate::app::schema_synth_build::{
    CdkBuildRouted, record_lock_evidence_with_manifest, route_cdk_build,
};

pub(crate) fn handle_fork(id: &str, json: bool) -> crate::Result<CommandOutput<()>> {
    let repo_root = current_utf8_dir()?;
    let scope = DistributionScope::project(&repo_root);

    let manifest = scope.read_manifest()?;
    let alias = ctx_traits_io::distribution::resolve_installed_operand(&manifest, id)
        .map_err(|_| crate::Error::Command {
            message: format!(
                "{id:?} is not an installed dependency; run `ctx traits dependency add` first"
            ),
        })?
        .to_string();

    let lock = scope.read_lock()?;
    let lock_entry = lock
        .as_ref()
        .and_then(|lock| lock.package_entry(&alias))
        .cloned()
        .ok_or_else(|| crate::Error::Command {
            message: format!(
                "{alias:?} has no lock evidence; run `ctx traits dependency install` first"
            ),
        })?;
    let forked_from_digest = default_variant_trait(&lock_entry.traits)?
        .canonical_digest
        .clone();

    let vendored_root = scope.vendored_package_root(&alias)?;
    if !vendored_root.is_dir() {
        return Err(crate::Error::Command {
            message: format!(
                "{vendored_root} does not exist; run `ctx traits dependency install` first"
            ),
        });
    }
    let vendored_manifest_path = ctx_traits_io::layout::package_manifest_path(&vendored_root);
    let vendored_manifest_text = ctx_traits_io::read::read_text(&vendored_manifest_path)?;
    let vendored_manifest = ctx_traits_core::manifest::decode_package_manifest(
        &vendored_manifest_text,
        vendored_manifest_path.as_str(),
    )?
    .ok_or_else(|| crate::Error::Command {
        message: format!("{vendored_manifest_path} has no [package] table; not a forkable package"),
    })?;
    let package_id = vendored_manifest.package.id.clone();
    let forked_from_version = vendored_manifest.package.version.clone();

    if !vendored_root.join("source").is_dir() {
        return Err(crate::Error::Command {
            message: format!(
                "{vendored_root} has no source/ directory; fork requires CDK authoring source \
                 (this dependency was vendored without it)"
            ),
        });
    }

    let package = ctx_traits_io::layout::TraitPackageRoot::new(&repo_root, &package_id)
        .map_err(ctx_traits_io::Error::from)?;
    // Same ordering `ctx traits init` uses: the authored root
    // (`.ctx/traits/authored`) is not guaranteed to exist yet in a project
    // that has only ever `dependency add`ed packages, and `claim_root`'s
    // exclusive `mkdir` is not recursive.
    ctx_traits_io::package_scaffold::ensure_dir(
        &ctx_traits_io::layout::trait_authoring_root_path(&repo_root),
        "trait authoring root",
    )?;
    if !ctx_traits_io::package_scaffold::claim_root(package.root(), "fork target package root")? {
        return Err(crate::Error::Command {
            message: format!(
                "{} already exists; ctx traits fork never overwrites an existing authored package",
                package.root()
            ),
        });
    }

    match fork_after_claim(
        &scope,
        &alias,
        &vendored_root,
        package.root(),
        &package_id,
        &forked_from_version,
        &forked_from_digest,
    ) {
        Ok(report) => {
            match OutputMode::select(json, false) {
                OutputMode::Json => {
                    print_json_report(&fork_report_json(&report), "fork output")?;
                }
                OutputMode::Human(mode) => {
                    let panel = fork_report_panel(&report);
                    emit_human(false, &panel, mode, || Ok(()))?;
                }
            }
            Ok(CommandOutput::new(()))
        }
        Err(error) => Err(cleanup_authored_root_then_return(package.root(), error)),
    }
}

/// Remove the authored root fork just claimed, folding a cleanup failure
/// into the returned error instead of discarding it — a caller must learn
/// that the claimed root may still be on disk, not just that the fork
/// itself failed.
fn cleanup_authored_root_then_return(root: &Utf8Path, error: crate::Error) -> crate::Error {
    match std::fs::remove_dir_all(root.as_std_path()) {
        Ok(()) => error,
        Err(cleanup_error) => crate::Error::Command {
            message: format!(
                "{error}; additionally failed to remove {root} after the failed fork: \
                 {cleanup_error}"
            ),
        },
    }
}

/// The default-variant trait lock entry: the single entry when there is
/// exactly one, or the entry flagged `is_default_variant` for a family. The
/// same resolution rule
/// [`ctx_traits_io::project_lock::ResolvedPackageLockPaths::path_for_id`]
/// applies to a bare (variant-less) reference.
fn default_variant_trait(traits: &[TraitLockEntry]) -> crate::Result<&TraitLockEntry> {
    match traits {
        [] => Err(crate::Error::Command {
            message: "vendored lock entry has no trait evidence to fork from".to_string(),
        }),
        [single] => Ok(single),
        many => many
            .iter()
            .find(|t| t.is_default_variant)
            .ok_or_else(|| crate::Error::Command {
                message: "vendored lock entry declares multiple traits with no default variant \
                          flagged; cannot determine fork provenance"
                    .to_string(),
            }),
    }
}

/// A sibling scratch-file path for `manifest_path`, unique per
/// process/instant, matching `distribution::remove`'s own internal backup
/// naming shape. Used only to hold the projected post-detach manifest text
/// fork's pre-detach lock finalization reads from; never the real project
/// manifest.
fn fork_projection_scratch_path(manifest_path: &Utf8Path) -> camino::Utf8PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    manifest_path.with_file_name(format!(
        "{}.fork-projection-{}-{nanos}",
        manifest_path.file_name().unwrap_or("file"),
        std::process::id(),
    ))
}

/// A sibling scratch-directory path for `manifest_path`, unique per
/// process/instant, that lock finalization vendors unrelated project
/// dependencies into instead of the live `.ctx/traits/vendored` tree. Owned
/// from creation: every exit path below removes it wholesale and folds a
/// cleanup failure into the returned error, same as
/// [`fork_projection_scratch_path`]'s file.
fn fork_vendor_scratch_root(manifest_path: &Utf8Path) -> camino::Utf8PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    manifest_path.with_file_name(format!(
        ".fork-vendor-scratch-{}-{nanos}",
        std::process::id(),
    ))
}

/// Route the CDK build (single vs. family) and finalize the authored
/// package's lock against `scratch_manifest_path`'s projected post-detach
/// dependency set, returning the generated target paths, the lock path, and
/// the rebuilt canonical digest (the default variant's, for a family). The
/// only place either single or family lock evidence is written, so there is
/// exactly one sync pass — the family branch's digest lookup and its lock
/// finalization are the same write, not an interim live-manifest pass
/// followed by a second projected one.
///
/// `scratch_vendor_root` redirects where lock finalization physically
/// vendors *other* project dependencies while computing this trait's own
/// lock evidence: `dependency::sync` in write mode always vendors the full
/// merged project+package+trait dependency set, not just this package's own
/// declarations, so without redirection it would copy (and, for a changed
/// source, overwrite) unrelated dependencies straight into the live
/// project's `.ctx/traits/vendored` — a side effect on packages fork never
/// touches, outside the authored root, that a later failure (trust read,
/// detach) could not roll back. Every lock entry's recorded digests come
/// from the dependency's own loaded source, never from the vendored copy
/// (`ctx_traits_io::dependency::lock_entry_for`), so redirecting the copy
/// destination changes nothing about the resulting lock content.
fn fork_build_and_lock(
    source_path: &str,
    authored_root: &Utf8Path,
    scratch_manifest_path: &Utf8Path,
    scratch_vendor_root: &Utf8Path,
) -> crate::Result<(Vec<String>, String, String)> {
    match route_cdk_build(source_path, "toml", None)? {
        CdkBuildRouted::Single(evidence) => {
            let canonical_digest = evidence
                .outcome
                .response
                .provenance
                .canonical_digest
                .as_str()
                .to_string();
            record_lock_evidence_with_manifest(
                &evidence.target_path,
                false,
                Some(scratch_manifest_path),
                Some(scratch_vendor_root),
            )?;
            Ok((
                vec![evidence.target_path.to_string()],
                ctx_traits_io::layout::package_lock_path(authored_root).to_string(),
                canonical_digest,
            ))
        }
        CdkBuildRouted::Family(family_report) => {
            for variant in &family_report.variants {
                record_lock_evidence_with_manifest(
                    Utf8Path::new(&variant.target),
                    false,
                    Some(scratch_manifest_path),
                    Some(scratch_vendor_root),
                )?;
            }
            let manifest_path = ctx_traits_io::layout::package_manifest_path(authored_root);
            let family_table = ctx_traits_io::family_manifest::read_family_table(&manifest_path)?
                .ok_or_else(|| crate::Error::Command {
                message: format!("{manifest_path} declares no [family] after build"),
            })?;
            let document =
                ctx_traits_io::lockfile::read_lockfile(authored_root)?.ok_or_else(|| {
                    crate::Error::Command {
                        message: format!("no trait.lock written for {authored_root} after build"),
                    }
                })?;
            let default_entry = document
                .traits
                .iter()
                .find(|entry| entry.variant.as_deref() == Some(family_table.default.as_str()))
                .ok_or_else(|| crate::Error::Command {
                    message: format!(
                        "trait.lock has no entry for default variant {:?}",
                        family_table.default
                    ),
                })?;
            let canonical_digest = default_entry
                .digests
                .as_ref()
                .and_then(|digests| digests.canonical.clone())
                .ok_or_else(|| crate::Error::Command {
                    message: "default-variant lock entry has no canonical digest".to_string(),
                })?;
            Ok((
                family_report
                    .variants
                    .iter()
                    .map(|variant| variant.target.clone())
                    .collect(),
                ctx_traits_io::layout::package_lock_path(authored_root).to_string(),
                canonical_digest,
            ))
        }
    }
}

/// `std::fs::remove_dir_all`, tolerant of the directory never having been
/// created at all: lock finalization only creates `scratch_vendor_root` if
/// the projected dependency set is non-empty, so a fork with no project
/// dependencies leaves nothing to clean up and that must not read as a
/// cleanup failure.
fn remove_dir_all_if_exists(root: &Utf8Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(root.as_std_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// `std::fs::remove_file`, tolerant of the file never having been created:
/// a failed write may have created nothing at all.
fn remove_file_if_exists(path: &Utf8Path) -> std::io::Result<()> {
    match std::fs::remove_file(path.as_std_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Write the projected post-detach manifest to `path`, cleaning up on
/// failure: a write that fails partway can leave a partial file behind, and
/// this scratch file is owned from its very first write attempt, not just
/// from a successful one.
fn write_scratch_manifest(path: &Utf8Path, text: &str) -> crate::Result<()> {
    match ctx_traits_io::write::write_text(path, text) {
        Ok(()) => Ok(()),
        Err(error) => match remove_file_if_exists(path) {
            Ok(()) => Err(error.into()),
            Err(cleanup_error) => Err(crate::Error::Command {
                message: format!(
                    "{error}; additionally failed to remove {path} after a failed write: \
                     {cleanup_error}"
                ),
            }),
        },
    }
}

/// Fold an `std::io::Error` cleanup failure into `result`'s error, rather
/// than discarding it: a caller must learn that a cleanup step (e.g.
/// removing the fork projection scratch file) also failed, not just that
/// the primary operation did.
fn fold_cleanup_result<T>(
    result: crate::Result<T>,
    cleanup: std::io::Result<()>,
    cleanup_label: &str,
) -> crate::Result<T> {
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup_error)) => Err(crate::Error::Command {
            message: format!("fork succeeded but failed to {cleanup_label}: {cleanup_error}"),
        }),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(crate::Error::Command {
            message: format!("{error}; additionally failed to {cleanup_label}: {cleanup_error}"),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn fork_after_claim(
    scope: &DistributionScope,
    alias: &str,
    vendored_root: &Utf8Path,
    authored_root: &Utf8Path,
    package_id: &str,
    forked_from_version: &str,
    forked_from_digest: &str,
) -> crate::Result<ForkReport> {
    ctx_traits_io::fork::copy_authoring_subset(vendored_root, authored_root)?;

    let source_path = ctx_traits_io::layout::package_cdk_source_path(authored_root)
        .map_err(ctx_traits_io::Error::from)?
        .ok_or_else(|| crate::Error::Command {
            message: format!("{authored_root} has no CDK authoring source after copy"),
        })?;

    // Every lock write below — the family branch's digest-discovery sync
    // included — must observe the *post-detach* dependency set:
    // `dependency::sync` merges the live project-level `[dependencies]` into
    // whatever `trait_file` it is pointed at, and the forked alias (still
    // installed at this point) is one of them. Syncing against the live
    // project manifest would resolve that declaration and re-vendor
    // `alias`'s own installed tree from its live source mid-fork, before the
    // fork has even succeeded. So the post-detach manifest is projected into
    // a scratch file — never the real project manifest — and owned from its
    // first write: every exit path below removes it and folds a cleanup
    // failure into the returned error, matching this function's own
    // "residue-free on failure" contract for the authored root.
    //
    // The same sync also vendors *every* merged project dependency, not just
    // this package's own — so its physical vendor-copy destination is
    // likewise redirected to an owned scratch directory
    // ([`fork_vendor_scratch_root`]), never the live `.ctx/traits/vendored`
    // tree, and removed on every exit path the same way.
    let project_manifest_path = scope.manifest_path("toml");
    let projected_manifest_text =
        ctx_traits_io::distribution::manifest_text_without_dependency(scope, alias)?;
    let scratch_manifest_path = fork_projection_scratch_path(&project_manifest_path);
    write_scratch_manifest(&scratch_manifest_path, &projected_manifest_text)?;
    let scratch_vendor_root = fork_vendor_scratch_root(&project_manifest_path);

    let build_result = fork_build_and_lock(
        source_path.as_str(),
        authored_root,
        &scratch_manifest_path,
        &scratch_vendor_root,
    );
    let manifest_cleanup = std::fs::remove_file(scratch_manifest_path.as_std_path());
    let vendor_cleanup = remove_dir_all_if_exists(&scratch_vendor_root);
    let (generated, lock, rebuilt_canonical_digest) = fold_cleanup_result(
        fold_cleanup_result(
            build_result,
            manifest_cleanup,
            &format!("remove scratch projection {scratch_manifest_path}"),
        ),
        vendor_cleanup,
        &format!("remove scratch vendor directory {scratch_vendor_root}"),
    )?;

    let manifest_path = ctx_traits_io::layout::package_manifest_path(authored_root);
    ctx_traits_io::fork::write_forked_from(
        &manifest_path,
        ctx_traits_io::fork::ForkProvenance {
            id: package_id,
            version: forked_from_version,
            canonical_digest: forked_from_digest,
        },
    )?;

    // Trust resolution is a fallible read (trust-store decode can fail) with
    // no side effects; it must happen before the irreversible detach below
    // so a read failure here leaves the original dependency installed
    // instead of stranding the fork half-detached (the outer error handler
    // can only remove the authored root it claimed, not undo
    // `distribution::remove`).
    let trust = ctx_traits_io::lifecycle::resolve_trust_verdict_for_trait(
        package_id,
        &rebuilt_canonical_digest,
    )?;

    // The sole irreversible mutation in this transaction, run last: it is
    // already fully self-atomic (manifest declaration, project lock entry,
    // vendored tree, and the append-only Remove audit record all commit or
    // roll back together), so a failure here leaves the original dependency
    // installed with nothing further for fork to undo.
    let remove_report = ctx_traits_io::distribution::remove(scope, alias)?;

    Ok(ForkReport {
        id: package_id.to_string(),
        alias: alias.to_string(),
        forked_from_id: package_id.to_string(),
        forked_from_version: forked_from_version.to_string(),
        forked_from_digest: forked_from_digest.to_string(),
        package_manifest: manifest_path.to_string(),
        source_path: source_path.to_string(),
        generated,
        lock,
        rebuilt_canonical_digest,
        detached_package: remove_report.package,
        trust,
    })
}

struct ForkReport {
    id: String,
    alias: String,
    forked_from_id: String,
    forked_from_version: String,
    forked_from_digest: String,
    package_manifest: String,
    source_path: String,
    generated: Vec<String>,
    lock: String,
    rebuilt_canonical_digest: String,
    detached_package: String,
    trust: TrustVerdict,
}

fn fork_report_panel(report: &ForkReport) -> Panel {
    let (status_tone, status) = match report.trust {
        TrustVerdict::Verified => (RowTone::Pass, PanelStatus::Passed("passed".to_string())),
        TrustVerdict::Blocked => (RowTone::Fail, PanelStatus::Blocked("blocked".to_string())),
        TrustVerdict::Unreviewed => (RowTone::Default, PanelStatus::Passed("passed".to_string())),
    };
    Panel::new("ctx", "fork", status)
        .row(PanelRow::toned("id", &report.id, RowTone::Default))
        .row(PanelRow::toned("alias", &report.alias, RowTone::Default))
        .row(PanelRow::toned(
            "forked-from",
            format!(
                "{}@{} canonical {}",
                report.forked_from_id, report.forked_from_version, report.forked_from_digest
            ),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "package-manifest",
            report.package_manifest.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "source",
            report.source_path.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "generated",
            report.generated.join(", "),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "lock",
            report.lock.as_str(),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "detached",
            format!(
                "declaration, lock entry, and vendored tree removed for {} ({})",
                report.alias, report.detached_package
            ),
            RowTone::Default,
        ))
        .row(PanelRow::toned(
            "status",
            trust_status_line(&report.id, report.trust, "forked"),
            status_tone,
        ))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ForkReportJson<'a> {
    id: &'a str,
    alias: &'a str,
    forked_from_id: &'a str,
    forked_from_version: &'a str,
    forked_from_digest: &'a str,
    package_manifest: &'a str,
    source: &'a str,
    generated: &'a [String],
    lock: &'a str,
    rebuilt_canonical_digest: &'a str,
    detached_package: &'a str,
    trust: TrustVerdict,
}

fn fork_report_json(report: &ForkReport) -> ForkReportJson<'_> {
    ForkReportJson {
        id: &report.id,
        alias: &report.alias,
        forked_from_id: &report.forked_from_id,
        forked_from_version: &report.forked_from_version,
        forked_from_digest: &report.forked_from_digest,
        package_manifest: &report.package_manifest,
        source: &report.source_path,
        generated: &report.generated,
        lock: &report.lock,
        rebuilt_canonical_digest: &report.rebuilt_canonical_digest,
        detached_package: &report.detached_package,
        trust: report.trust,
    }
}
