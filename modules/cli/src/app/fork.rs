//! `ctx traits fork <id>` (0213): turn an installed vendored dependency at
//! `.ctx/traits/vendored/<id>` into an editable authored package at
//! `.ctx/traits/authored/<id>`.
//!
//! One transaction: copy the vendored authoring subset into a freshly
//! claimed authored root, rebuild it through the normal CDK build path
//! ([`crate::app::schema_synth_build`]), lock it, record forked-from
//! provenance, detach the vendored dependency (manifest declaration +
//! project lock entry + vendored tree, via
//! [`ctx_traits_io::distribution::remove`]), and report trust by canonical
//! digest. The authored root is claimed atomically first (the `create`
//! precedent); any failure after that point removes everything under it, so
//! a failed fork never leaves a half-copied authored package, and the
//! manifest/lock/vendored tree stay untouched.

use camino::Utf8Path;
use ctx_traits_core::project_lock::TraitLockEntry;
use ctx_traits_core::response::CommandOutput;
use ctx_traits_core::r#trait::TrustVerdict;
use ctx_traits_io::distribution::DistributionScope;

use crate::app::command_handlers::print_json_report;
use crate::app::lifecycle_reporting::current_utf8_dir;
use crate::app::new::trust_status_line;
use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};
use crate::app::schema_synth_build::{CdkBuildRouted, record_lock_evidence, route_cdk_build};

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
        Err(error) => {
            let _ = std::fs::remove_dir_all(package.root().as_std_path());
            Err(error)
        }
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

    // Route determination (single vs. family) and the digest needed for the
    // trust check below never touch `dependency::sync`'s project-dependency
    // merge, so both are safe to resolve before the alias is detached.
    let (generated, family_variant_targets, rebuilt_canonical_digest) =
        match route_cdk_build(source_path.as_str(), "toml", None)? {
            CdkBuildRouted::Single(evidence) => {
                let canonical_digest = evidence
                    .outcome
                    .response
                    .provenance
                    .canonical_digest
                    .as_str()
                    .to_string();
                (
                    vec![evidence.target_path.to_string()],
                    None,
                    canonical_digest,
                )
            }
            CdkBuildRouted::Family(family_report) => {
                // `route_cdk_build`'s family report does not carry each
                // variant's canonical digest, so an interim lock sync reads
                // it back off the just-written `trait.lock`. This first sync
                // still runs while the forked alias is a live project
                // dependency, so its `dependencies` rows are provisional —
                // the post-detach `record_lock_evidence` pass below re-syncs
                // every variant once more after detach to reconcile that
                // away; upsert-by-`(id, variant)` semantics make the second
                // pass authoritative.
                for variant in &family_report.variants {
                    record_lock_evidence(Utf8Path::new(&variant.target), false)?;
                }
                let manifest_path = ctx_traits_io::layout::package_manifest_path(authored_root);
                let family_table = ctx_traits_io::family_manifest::read_family_table(
                    &manifest_path,
                )?
                .ok_or_else(|| crate::Error::Command {
                    message: format!("{manifest_path} declares no [family] after build"),
                })?;
                let document =
                    ctx_traits_io::lockfile::read_lockfile(authored_root)?.ok_or_else(|| {
                        crate::Error::Command {
                            message: format!(
                                "no trait.lock written for {authored_root} after build"
                            ),
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
                let targets: Vec<camino::Utf8PathBuf> = family_report
                    .variants
                    .iter()
                    .map(|variant| camino::Utf8PathBuf::from(&variant.target))
                    .collect();
                (
                    family_report
                        .variants
                        .iter()
                        .map(|variant| variant.target.clone())
                        .collect(),
                    Some(targets),
                    canonical_digest,
                )
            }
        };

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

    let remove_report = ctx_traits_io::distribution::remove(scope, alias)?;

    // Lock finalization happens only now, after the alias is gone from
    // project configuration: `dependency::sync` merges the *current*
    // project-level `[dependencies]` into every package's own `trait.lock`,
    // so finalizing before detach would freeze a stale row for the very
    // dependency this command just removed. Running it here instead means
    // the authored package's lock reflects the post-detach dependency set —
    // the forked alias absent, everything else the package (or project)
    // still legitimately depends on still represented — while build-derived
    // pins (`sync_derived_dependency_locks`) are unaffected by detach and
    // stay correct either way.
    let lock = match &family_variant_targets {
        None => {
            record_lock_evidence(camino::Utf8Path::new(&generated[0]), false)?;
            ctx_traits_io::layout::package_lock_path(authored_root).to_string()
        }
        Some(targets) => {
            for target in targets {
                record_lock_evidence(target, false)?;
            }
            ctx_traits_io::layout::package_lock_path(authored_root).to_string()
        }
    };

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
