//! Dependency resolution and vendoring at the IO boundary.
//!
//! Core only plans dependency source actions. This module executes local,
//! npm, and Git package reads, vendors the package evidence under
//! `.ctx/traits/vendor` (generated, gitignored content — P446 — unaffected
//! by P426's runtime-state relocation), and computes digest layers for
//! lock/check. Git sources use a deterministic,
//! URL-keyed cache under this repository's global per-repository cache
//! root's `git` subfamily (`~/.config/ctx/cache/<repo-key>/git/`, P426): a
//! mutable fetch clone plus one immutable checkout directory per resolved
//! commit, so concurrent syncs resolving different revisions of the same URL
//! never read each other's checkout. The resolved commit is recorded as
//! `rev` on the dependency lock entry.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
use ctx_traits_core::manifest::{Dependency, ProjectManifest};
use ctx_traits_core::resource_plan::{
    BodyEvidence, DependencyResourceAvailability, DependencyResourceDecl, FileEvidence,
};
use ctx_traits_core::source_plan::{SourceAction, plan_dependency};
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct DependencyDeclaration {
    pub dependency: Dependency,
    pub base_dir: Utf8PathBuf,
    pub origin: String,
}

#[derive(Debug, Clone)]
pub struct ProjectDependencies {
    pub manifest_path: Option<Utf8PathBuf>,
    pub dependencies: Vec<DependencyDeclaration>,
}

#[derive(Debug, Clone)]
pub struct LoadedDependency {
    pub alias: String,
    pub id: String,
    pub version: String,
    pub root: Utf8PathBuf,
    pub manifest_path: Utf8PathBuf,
    pub trait_ref: ctx_traits_core::Trait,
    pub source_text: String,
    pub source_digest: Digest,
    pub canonical_digest: Digest,
    pub model_visible_digest: Digest,
    pub model_visible_text: String,
    pub resource_manifest_digest: Digest,
    pub file_evidence: Vec<FileEvidence>,
    /// Every readable resource body, regardless of trigger — used for
    /// qualified dependency-resource materialization and hidden-content
    /// auditing, which must see on-demand resources too.
    pub body_evidence: Vec<BodyEvidence>,
    /// The subset of `body_evidence` that actually feeds this trait's own
    /// model-view render (on-activation/template-reachable resources only).
    /// Digest computations over "what the model sees" must use this, not
    /// `body_evidence`.
    pub model_view_body_evidence: Vec<BodyEvidence>,
    pub body_read_warnings: Vec<crate::resource::ResourceReadWarning>,
    pub package_warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SyncReport {
    pub repo_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_manifest: Option<String>,
    pub lockfile: String,
    pub locked: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub dependencies: Vec<SyncDependencyReport>,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SyncDependencyReport {
    pub alias: String,
    pub id: String,
    pub requested_version: String,
    pub resolved_version: String,
    pub source_kind: String,
    pub source_path: String,
    pub vendored_path: String,
    pub source_digest: String,
    pub canonical_digest: String,
    pub model_visible_digest: String,
    pub resource_manifest_digest: String,
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TraitDependencyDeclarations {
    pub dependencies: Vec<DependencyDeclaration>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    Write,
    VerifyLocked,
}

#[derive(Debug, Clone)]
pub struct SyncRequest<'a> {
    pub repo_root: &'a Utf8Path,
    pub manifest_path: Option<&'a Utf8Path>,
    pub trait_file: Option<&'a Utf8Path>,
    pub mode: SyncMode,
}

pub fn sync(request: SyncRequest<'_>) -> crate::Result<SyncReport> {
    let Some(trait_file) = request.trait_file else {
        return invalid_input(
            "sync.file",
            "--file <trait.toml> is required because lock evidence is package-local",
        );
    };
    let loaded_trait = load_dependency_package("self", None, None, trait_file)?;
    let trait_root = loaded_trait.root.clone();
    if crate::layout::is_builtin_store_package_root(&trait_root) {
        return invalid_input(
            trait_file,
            "sync target is a built-in trait store package; built-in packages are read-only and are never synced or vendored",
        );
    }
    let project = project_dependencies(request.repo_root, request.manifest_path)?;
    let mut sync_warnings = Vec::new();
    let declarations = package_trait_dependency_declarations(&trait_root, trait_file)?;
    sync_warnings.extend(declarations.warnings);
    let trait_deps = declarations.dependencies;
    let manifest_deps = package_manifest_dependencies(&trait_root)?;
    let dependencies =
        merge_declarations([project.dependencies, manifest_deps, trait_deps].concat())?;
    let dependency_aliases = dependencies
        .iter()
        .map(|declaration| declaration.dependency.alias.clone())
        .collect::<BTreeSet<_>>();
    let mut lockfile =
        crate::lockfile::read_lockfile(&trait_root)?.unwrap_or_else(|| crate::lockfile::Document {
            metadata: Some(crate::lockfile::Metadata {
                schema_version: Some("0.1".to_string()),
                generated_by: Some("ctx traits sync".to_string()),
                generated_at: Some("not-recorded".to_string()),
            }),
            traits: Vec::new(),
            dependencies: Vec::new(),
            import: None,
        });
    if lockfile.metadata.is_none() {
        lockfile.metadata = Some(crate::lockfile::Metadata {
            schema_version: Some("0.1".to_string()),
            generated_by: Some("ctx traits sync".to_string()),
            generated_at: Some("not-recorded".to_string()),
        });
    }

    // Establish the nested-ignore invariant before any vendor/lock mutation
    // below, not after: if this fails, sync must return an error having
    // changed nothing, rather than leaving vendored/locked state committed
    // to a caller that sees a failure (P446).
    if request.mode == SyncMode::Write && !dependencies.is_empty() {
        crate::gitignore::ensure_nested_gitignore(request.repo_root)?;
    }

    let mut reports = Vec::new();
    let mut dependency_body_evidence = Vec::new();
    let mut dependency_resource_decls = Vec::new();
    for declaration in dependencies {
        let locked_rev = match request.mode {
            SyncMode::VerifyLocked => lockfile
                .dependency_entry(&declaration.dependency.alias)
                .and_then(|entry| entry.rev.as_deref()),
            SyncMode::Write => None,
        };
        let resolved = resolve_source(request.repo_root, &declaration, locked_rev)?;
        let resolved_rev = resolved.resolved_rev.clone();
        let loaded = load_dependency_package(
            &declaration.dependency.alias,
            Some(&declaration.dependency.id),
            Some(&declaration.dependency.version),
            &resolved.manifest_path,
        )?;
        if crate::layout::is_builtin_store_package_root(&loaded.root) {
            return invalid_input(
                &resolved.manifest_path,
                format!(
                    "dependency {:?} source is inside the built-in trait store; built-in packages are read-only and are never vendored",
                    declaration.dependency.alias
                ),
            );
        }
        let vendored_root = crate::layout::vendored_dependency_root(
            request.repo_root,
            &declaration.dependency.alias,
        )?;
        let vendored_path = format!(
            "{}/{}",
            crate::layout::trait_vendor_root(),
            declaration.dependency.alias
        );
        for resource_id in qualified_dependency_resource_ids(
            &loaded_trait.trait_ref,
            &declaration.dependency.alias,
        )? {
            if let Some(body) = loaded
                .body_evidence
                .iter()
                .find(|body| body.resource_id == resource_id)
            {
                let mut body = body.clone();
                body.resource_id = format!("{}/{resource_id}", declaration.dependency.alias);
                dependency_body_evidence.push(body);
            }
            // Declaration metadata (render mode, presentation path) is
            // carried independently of whether a body was scanned, so a
            // reference-mode dependency resource — or one whose body scan
            // produced no text — still gets a plan entry and a usable
            // pointer.
            if let Some(decl) = dependency_resource_decl(&loaded, &resource_id)? {
                dependency_resource_decls.push(decl);
            }
        }
        let (status, mut warnings) = match request.mode {
            SyncMode::Write => {
                let changed = copy_package_subset(&loaded.root, &vendored_root)?;
                let entry = lock_entry_for(
                    &declaration.dependency,
                    &loaded,
                    &vendored_path,
                    resolved_rev.clone(),
                )?;
                lockfile.upsert_dependency_entry(entry);
                (
                    if changed { "vendored" } else { "unchanged" }.to_string(),
                    Vec::new(),
                )
            }
            SyncMode::VerifyLocked => verify_locked_dependency(
                request.repo_root,
                &lockfile,
                &declaration.dependency,
                &loaded,
                &vendored_root,
                resolved_rev.as_deref(),
            )?,
        };
        warnings.extend(resolved.warnings);
        warnings.extend(loaded.package_warnings.clone());
        warnings.sort();
        warnings.dedup();
        reports.push(SyncDependencyReport {
            alias: declaration.dependency.alias.clone(),
            id: declaration.dependency.id.clone(),
            requested_version: declaration.dependency.version.clone(),
            resolved_version: loaded.version.clone(),
            source_kind: resolved.source_kind,
            source_path: resolved.source_path,
            vendored_path,
            source_digest: loaded.source_digest.as_str().to_string(),
            canonical_digest: loaded.canonical_digest.as_str().to_string(),
            model_visible_digest: loaded.model_visible_digest.as_str().to_string(),
            resource_manifest_digest: loaded.resource_manifest_digest.as_str().to_string(),
            status,
            warnings,
        });
    }

    let model_visible_digest = model_visible_digest_with_dependencies(
        &loaded_trait,
        dependency_body_evidence,
        dependency_resource_decls,
    )?;
    let variant = loaded_trait.trait_ref.variant.as_deref();
    let trait_lock_matches = lockfile
        .trait_entry(&loaded_trait.id, variant)
        .is_some_and(|entry| locked_trait_matches(entry, &loaded_trait, &model_visible_digest));
    if request.mode == SyncMode::Write {
        upsert_trait_lock_entry(&mut lockfile, &loaded_trait, &model_visible_digest);
    } else if !trait_lock_matches {
        sync_warnings.push(format!(
            "trait:{} lock evidence is missing or stale",
            loaded_trait.id
        ));
    }

    reports.sort_by(|a, b| a.alias.cmp(&b.alias));
    let passed = reports
        .iter()
        .all(|report| matches!(report.status.as_str(), "vendored" | "unchanged" | "locked"))
        && (request.mode == SyncMode::Write || trait_lock_matches);
    if request.mode == SyncMode::Write {
        lockfile
            .dependencies
            .retain(|entry| dependency_aliases.contains(&entry.alias));
        crate::lockfile::write_lockfile(&trait_root, &mut lockfile)?;
    }
    sync_warnings.sort();
    sync_warnings.dedup();
    Ok(SyncReport {
        repo_root: request.repo_root.to_string(),
        project_manifest: project.manifest_path.map(|path| path.to_string()),
        lockfile: crate::lockfile::lockfile_path(&trait_root).to_string(),
        locked: request.mode == SyncMode::VerifyLocked,
        warnings: sync_warnings,
        dependencies: reports,
        passed,
    })
}

fn upsert_trait_lock_entry(
    document: &mut crate::lockfile::Document,
    loaded: &LoadedDependency,
    model_visible_digest: &Digest,
) {
    let variant = loaded.trait_ref.variant.as_deref();
    let policy_manifest = document
        .trait_entry(&loaded.id, variant)
        .and_then(|entry| entry.digests.as_ref())
        .and_then(|digests| digests.policy_manifest.clone());
    let digests = crate::lockfile::LockDigests {
        source: Some(loaded.source_digest.as_str().to_string()),
        canonical: Some(loaded.canonical_digest.as_str().to_string()),
        model_visible: Some(model_visible_digest.as_str().to_string()),
        policy_manifest,
        resource_manifest: Some(loaded.resource_manifest_digest.as_str().to_string()),
    };
    if let Some(entry) = document.trait_entry_mut(&loaded.id, variant) {
        entry.requested_version = Some(loaded.version.clone());
        entry.resolved_version = Some(loaded.version.clone());
        entry.digests = Some(digests);
        return;
    }
    document.traits.push(crate::lockfile::LockTraitEntry {
        id: loaded.id.clone(),
        variant: variant.map(str::to_string),
        requested_version: Some(loaded.version.clone()),
        resolved_version: Some(loaded.version.clone()),
        digests: Some(digests),
        ..Default::default()
    });
}

fn locked_trait_matches(
    entry: &crate::lockfile::LockTraitEntry,
    loaded: &LoadedDependency,
    model_visible_digest: &Digest,
) -> bool {
    entry.resolved_version.as_deref() == Some(loaded.version.as_str())
        && entry.source_digest() == Some(loaded.source_digest.as_str())
        && entry.canonical_digest() == Some(loaded.canonical_digest.as_str())
        && entry.model_visible_digest() == Some(model_visible_digest.as_str())
        && entry.resource_manifest_digest() == Some(loaded.resource_manifest_digest.as_str())
}

fn model_visible_digest_with_dependencies(
    loaded: &LoadedDependency,
    dependency_body_evidence: Vec<BodyEvidence>,
    dependency_resource_decls: Vec<DependencyResourceDecl>,
) -> crate::Result<Digest> {
    let roots = crate::resource::resolve_resource_roots(&loaded.root, &loaded.trait_ref.resources)?;
    let manifest = crate::resource::digest_resources(
        &roots,
        loaded.trait_ref.id.as_str(),
        &loaded.trait_ref.resources,
    )?;
    let mut body_evidence = loaded.model_view_body_evidence.clone();
    body_evidence.extend(dependency_body_evidence);
    let mut warnings = resource_read_warning_strings(&manifest.warnings);
    warnings.extend(resource_read_warning_strings(&loaded.body_read_warnings));
    Ok(
        ctx_traits_core::render::plan_render_with_resource_body_evidence(
            &loaded.trait_ref,
            ctx_traits_core::render::ExtendedRenderProfile::AgentSkills,
            loaded.source_digest.as_str(),
            ctx_traits_core::render::ResourceEvidenceInputs {
                file_evidence: &loaded.file_evidence,
                body_evidence: &body_evidence,
                dependency_resources: &dependency_resource_decls,
                manifest_digest: Some(manifest.manifest_digest.as_str()),
                read_warnings: warnings,
            },
        )
        .model_view
        .content_digest,
    )
}

/// Build the qualified declaration metadata for one `<alias>/<resource_id>`
/// dependency resource, read directly from the dependency's own resource
/// declaration — independent of whether a body was scanned for it. Returns
/// `Ok(None)` only if the dependency no longer declares `resource_id` (a
/// stale qualified reference in the parent), which is reported elsewhere as
/// an unresolved reference, not fabricated here.
///
/// The presentation path, when the resource declares one, is resolved
/// against the dependency's *actual loaded root* (`loaded.root`) using the
/// same safe resolver every other presentation path in this codebase uses
/// (`crate::resource::resolve_resource_roots` + `crate::resource::presentation_path`).
/// It is never synthesized by string-concatenating the alias — the alias is
/// only a logical name declared in the parent's `[dependencies]` table and
/// has no relationship to where the dependency package actually lives on
/// disk (a local path, an npm package fetched into this repo's registry
/// cache, or a git checkout under `.ctx/cache/git/...`). Resolving against
/// the real root
/// means the emitted path always names a file that exists when the
/// dependency is actually vendored/resolved at that root.
///
/// The single constructor for `DependencyResourceDecl`: `sync()` and every
/// CLI caller that materializes a qualified dependency resource (check,
/// drift, render) call this instead of re-deriving path/render from a
/// dependency's `Resource` declaration independently.
pub fn dependency_resource_decl(
    loaded: &LoadedDependency,
    resource_id: &str,
) -> crate::Result<Option<DependencyResourceDecl>> {
    let Some(resource) = loaded
        .trait_ref
        .resources
        .iter()
        .find(|resource| resource.id == resource_id)
    else {
        return Ok(None);
    };
    let (path, availability) = match resource.path.as_deref() {
        Some(relative) => {
            let roots = crate::resource::resolve_resource_roots(
                &loaded.root,
                std::slice::from_ref(resource),
            )?;
            let presented = crate::resource::presentation_path(&roots, resource, relative)?;
            let availability = map_presentation_status(presented.status);
            // A verified presentation is path *and* availability together:
            // only `Available` may become an openable reference. Missing,
            // symlinked, and special-file paths are withheld here — never
            // handed to a caller as something to open — and reported instead
            // via `availability`, exactly like a local reference resource's
            // status is reported through `PresentationStatus`.
            //
            // The presented pointer joins the dependency's own (uncanonicalized)
            // root with the already-validated relative path — never
            // `presented.path`, which the safety resolver builds from a
            // `canonicalize()`d root purely to defeat TOCTOU/symlink escapes
            // during the read/availability check. Reusing that canonicalized,
            // machine-absolute path here would leak the invoking checkout's
            // filesystem location into model-visible text (and any digest
            // over it), breaking the same byte-stability every other
            // resource path in this render already holds.
            let path = matches!(availability, DependencyResourceAvailability::Available)
                .then(|| loaded.root.join(relative).to_string());
            (path, Some(availability))
        }
        None => (None, None),
    };
    Ok(Some(DependencyResourceDecl {
        resource_id: format!("{}/{resource_id}", loaded.alias),
        path,
        availability,
        render: resource.effective_render(),
    }))
}

/// Map the IO safety resolver's presentation status onto its core-owned
/// mirror, so dependency evidence can carry availability across the core/IO
/// boundary without core depending on IO types.
fn map_presentation_status(
    status: crate::resource::PresentationStatus,
) -> DependencyResourceAvailability {
    match status {
        crate::resource::PresentationStatus::Available => DependencyResourceAvailability::Available,
        crate::resource::PresentationStatus::Missing => DependencyResourceAvailability::Missing,
        crate::resource::PresentationStatus::Symlink => DependencyResourceAvailability::Symlink,
        // Dependency-vendored resources are always files; a directory here
        // stays unavailable (the directory-shaped task board is a repo-root
        // resource declared directly, never vendored through a dependency).
        crate::resource::PresentationStatus::SpecialFile
        | crate::resource::PresentationStatus::Directory => {
            DependencyResourceAvailability::SpecialFile
        }
    }
}

fn qualified_dependency_resource_ids(
    trait_ref: &ctx_traits_core::Trait,
    alias: &str,
) -> crate::Result<BTreeSet<String>> {
    let value =
        serde_json::to_value(trait_ref).map_err(|source| crate::parse::Error::JsonSerialize {
            context: "serialize trait for dependency resource scan".to_string(),
            source,
        })?;
    let mut ids = BTreeSet::new();
    collect_qualified_dependency_resource_ids(&value, alias, None, &mut ids);
    Ok(ids)
}

fn collect_qualified_dependency_resource_ids(
    value: &serde_json::Value,
    alias: &str,
    field: Option<&str>,
    ids: &mut BTreeSet<String>,
) {
    match value {
        serde_json::Value::String(text) => {
            if field.is_some_and(is_prose_ref_scan_field) {
                return;
            }
            let ref_text = text
                .strip_prefix('[')
                .and_then(|inner| inner.strip_suffix(']'))
                .unwrap_or(text);
            let Ok(reference) = ctx_traits_core::reference::Reference::parse(ref_text) else {
                return;
            };
            if reference.kind() != ctx_traits_core::reference::Kind::Resource {
                return;
            }
            if let ctx_traits_core::reference::Path::Qualified { namespace, id, .. } =
                reference.ref_path()
                && namespace == alias
            {
                ids.insert(id.to_string());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_qualified_dependency_resource_ids(item, alias, field, ids);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                collect_qualified_dependency_resource_ids(item, alias, Some(key), ids);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn is_prose_ref_scan_field(field: &str) -> bool {
    matches!(
        field,
        "description" | "summary" | "hint" | "title" | "text" | "reason" | "name"
    )
}

pub fn project_dependencies(
    repo_root: &Utf8Path,
    explicit_manifest: Option<&Utf8Path>,
) -> crate::Result<ProjectDependencies> {
    let Some((manifest_path, manifest)) = read_project_manifest(repo_root, explicit_manifest)?
    else {
        return Ok(ProjectDependencies {
            manifest_path: None,
            dependencies: Vec::new(),
        });
    };
    // Project dependency paths are repository-relative, not relative to `.ctx`.
    let base_dir = repo_root.to_path_buf();
    let dependencies = manifest
        .dependencies
        .into_iter()
        .map(|dependency| DependencyDeclaration {
            dependency,
            base_dir: base_dir.clone(),
            origin: manifest_path.to_string(),
        })
        .collect();
    Ok(ProjectDependencies {
        manifest_path: Some(manifest_path),
        dependencies,
    })
}

pub fn trait_dependency_declarations(
    trait_file: &Utf8Path,
) -> crate::Result<TraitDependencyDeclarations> {
    let (trait_ref, trait_root, _, canonical_digest) = crate::run::load_trait(trait_file.as_str())?;
    let (status, trust) = crate::lifecycle::resolve_named(
        &trait_root,
        trait_ref.id.as_str(),
        canonical_digest.as_str(),
    )?;
    let mut warnings = Vec::new();
    if matches!(status, ctx_traits_core::manifest::PackageStatus::Draft)
        || !matches!(trust, ctx_traits_core::r#trait::TrustVerdict::Verified)
    {
        warnings.push(format!(
            "sync lifecycle gate: trait file {trait_file} has status={} machine-trust={}; dependency resolution is jailed, write-bounded, and depth-1 only",
            status.display_name(),
            trust.display_name(),
        ));
    }
    Ok(TraitDependencyDeclarations {
        dependencies: trait_ref
            .dependencies
            .into_iter()
            .map(|dependency| DependencyDeclaration {
                dependency,
                base_dir: trait_root.clone(),
                origin: trait_file.to_string(),
            })
            .collect(),
        warnings,
    })
}

fn package_trait_dependency_declarations(
    trait_root: &Utf8Path,
    selected_trait_file: &Utf8Path,
) -> crate::Result<TraitDependencyDeclarations> {
    let package_manifest = crate::layout::package_manifest_path(trait_root);
    let trait_files = match crate::family_manifest::read_family_table(&package_manifest)? {
        Some(family) => family
            .variants
            .into_values()
            .map(|variant| trait_root.join(variant.relative_path))
            .collect(),
        None => vec![selected_trait_file.to_path_buf()],
    };
    let mut dependencies = Vec::new();
    let mut warnings = Vec::new();
    for trait_file in trait_files {
        let declarations = trait_dependency_declarations(&trait_file)?;
        dependencies.extend(declarations.dependencies);
        warnings.extend(declarations.warnings);
    }
    Ok(TraitDependencyDeclarations {
        dependencies,
        warnings,
    })
}

pub fn declarations_for_trait(
    repo_root: &Utf8Path,
    trait_root: &Utf8Path,
    trait_ref: &ctx_traits_core::Trait,
) -> crate::Result<Vec<DependencyDeclaration>> {
    let project = project_dependencies(repo_root, None)?;
    let manifest_deps = package_manifest_dependencies(trait_root)?;
    let trait_deps = trait_ref
        .dependencies
        .iter()
        .cloned()
        .map(|dependency| DependencyDeclaration {
            dependency,
            base_dir: trait_root.to_path_buf(),
            origin: format!("{trait_root}/trait.toml"),
        })
        .collect::<Vec<_>>();
    merge_declarations([project.dependencies, manifest_deps, trait_deps].concat())
}

/// Dependency declarations from a package's root `trait.toml` manifest.
///
/// Absent manifests and legacy flat canonical documents (no `[package]`
/// table) contribute nothing; declared `[dependencies]` entries resolve
/// relative to the package root, like trait-declared dependencies.
pub fn package_manifest_dependencies(
    trait_root: &Utf8Path,
) -> crate::Result<Vec<DependencyDeclaration>> {
    let manifest_path = crate::layout::package_manifest_path(trait_root);
    if !manifest_path.is_file() {
        return Ok(Vec::new());
    }
    let text = crate::read::read_text(&manifest_path)?;
    let Some(manifest) =
        ctx_traits_core::manifest::decode_package_manifest(&text, manifest_path.as_str())?
    else {
        return Ok(Vec::new());
    };
    Ok(manifest
        .dependencies()?
        .into_iter()
        .map(|dependency| DependencyDeclaration {
            dependency,
            base_dir: trait_root.to_path_buf(),
            origin: manifest_path.to_string(),
        })
        .collect())
}

pub fn merge_declarations(
    declarations: Vec<DependencyDeclaration>,
) -> crate::Result<Vec<DependencyDeclaration>> {
    let mut by_alias: BTreeMap<String, DependencyDeclaration> = BTreeMap::new();
    for declaration in declarations {
        let alias = declaration.dependency.alias.clone();
        if let Some(existing) = by_alias.get(&alias) {
            if existing.dependency != declaration.dependency {
                return invalid_input(
                    &declaration.origin,
                    format!(
                        "dependency alias {alias:?} is declared with conflicting source/version data"
                    ),
                );
            }
            continue;
        }
        by_alias.insert(alias, declaration);
    }
    Ok(by_alias.into_values().collect())
}

pub fn load_vendored_dependency(
    repo_root: &Utf8Path,
    alias: &str,
) -> crate::Result<Option<LoadedDependency>> {
    let manifest_path = crate::layout::vendored_dependency_manifest(repo_root, alias)?;
    if !manifest_path.exists() {
        return Ok(None);
    }
    load_dependency_package(alias, None, None, &manifest_path).map(Some)
}

pub fn load_dependency_package(
    alias: &str,
    expected_id: Option<&str>,
    requested_version: Option<&str>,
    manifest_path: &Utf8Path,
) -> crate::Result<LoadedDependency> {
    let encoding = ctx_traits_core::encoding::Encoding::from_path(manifest_path)?;
    let source_text = crate::read::read_text(manifest_path)?;
    let (trait_ref, decode_warnings) =
        ctx_traits_core::encoding::decode_trait_with_warnings(encoding, &source_text)?;
    let mut package_warnings = Vec::new();
    for warning in &decode_warnings {
        package_warnings.push(format!("dependency:{alias} {warning}"));
    }
    if let Some(expected_id) = expected_id
        && trait_ref.id.as_str() != expected_id
    {
        return invalid_input(
            manifest_path,
            format!(
                "dependency {alias:?} expected trait id {expected_id:?}, found {:?}",
                trait_ref.id.as_str()
            ),
        );
    }
    if let Some(requested_version) = requested_version
        && trait_ref.version.as_str() != requested_version
    {
        package_warnings.push(format!(
                "dependency:{alias} requested version {requested_version} resolved {}; explicit sync records the resolved digest/version, locked verification reports drift",
                trait_ref.version.as_str()
            ));
    }
    if !trait_ref.dependencies.is_empty() {
        package_warnings.push(nested_dependency_warning(
            alias,
            trait_ref.dependencies.len(),
        ));
    }
    let root = crate::layout::package_root_for_manifest(manifest_path).ok_or_else(|| {
        crate::environment::Error::Filesystem {
            path: manifest_path.to_string(),
            source: std::io::Error::other("dependency trait manifest has no package root"),
        }
    })?;
    let roots = crate::resource::resolve_resource_roots(root, &trait_ref.resources)?;
    // Package-owned resources ONLY.
    //
    // A `root = "repo"` resource is an input the CONSUMING project supplies —
    // the `.internal/tasks` board, for one — not package content. Its
    // bytes differ in every repository and are absent in a fresh one, so
    // digesting it here produced lock evidence that could never be reproduced
    // anywhere but the producer's own checkout: installing `implement` into
    // another project vendored correctly, then failed `trust approve` with
    // "vendored content does not match locked evidence", permanently. A
    // package's integrity evidence can only cover the bytes the package ships.
    //
    // This narrows what the PACKAGE LOCK pins. The owning repository's own
    // `resource-manifest` drift check still digests every declared resource,
    // repo-rooted included — there, the repo IS the producer and those files
    // are legitimately part of what changed.
    let package_owned: Vec<ctx_traits_core::r#trait::Resource> = trait_ref
        .resources
        .iter()
        .filter(|resource| resource.root == ctx_traits_core::r#trait::ResourceRoot::Package)
        .cloned()
        .collect();
    let manifest =
        crate::resource::digest_resources(&roots, trait_ref.id.as_str(), &package_owned)?;
    let file_evidence = file_evidence_from_manifest(&manifest);
    // Full body set: needed so a parent trait's qualified reference to an
    // on-demand package resource can still be materialized, and so
    // hidden-content auditing covers every package resource — not just the
    // subset that feeds this dependency's own model-view render below.
    let (body_evidence, body_read_warnings) =
        crate::resource::scan_all_resource_bodies(&roots, &trait_ref)?;
    let (model_view_body_evidence, _) = crate::resource::scan_resource_bodies(&roots, &trait_ref)?;
    let source_digest = Digest::source(&source_text);
    let canonical_digest = ctx_traits_core::digest::canonical_digest(&trait_ref)?;
    let render_plan = ctx_traits_core::render::plan_render_with_resource_body_evidence(
        &trait_ref,
        ctx_traits_core::render::ExtendedRenderProfile::AgentSkills,
        source_digest.as_str(),
        ctx_traits_core::render::ResourceEvidenceInputs {
            file_evidence: &file_evidence,
            body_evidence: &model_view_body_evidence,
            dependency_resources: &[],
            manifest_digest: Some(manifest.manifest_digest.as_str()),
            read_warnings: resource_read_warning_strings(&manifest.warnings),
        },
    );
    Ok(LoadedDependency {
        alias: alias.to_string(),
        id: trait_ref.id.as_str().to_string(),
        version: trait_ref.version.as_str().to_string(),
        root: root.to_path_buf(),
        manifest_path: manifest_path.to_path_buf(),
        trait_ref,
        source_text,
        source_digest,
        canonical_digest,
        model_visible_digest: render_plan.model_view.content_digest,
        model_visible_text: render_plan.model_view.full_text,
        resource_manifest_digest: manifest.manifest_digest,
        file_evidence,
        body_evidence,
        model_view_body_evidence,
        body_read_warnings,
        package_warnings,
    })
}

pub fn nested_dependency_warning(alias: &str, count: usize) -> String {
    format!(
        "dependency:{alias} declares {count} nested dependencies; nested resolution is not performed"
    )
}

pub fn file_evidence_from_manifest(
    manifest: &crate::resource::ResourceManifestDigest,
) -> Vec<FileEvidence> {
    manifest
        .file_digests
        .iter()
        .map(|file| {
            ctx_traits_core::resource_plan::file_evidence_from_io(
                &file.resource_id,
                Some(&file.digest),
                file.byte_size,
                file.is_binary,
                false,
                false,
                false,
            )
        })
        .collect()
}

fn read_project_manifest(
    repo_root: &Utf8Path,
    explicit_manifest: Option<&Utf8Path>,
) -> crate::Result<Option<(Utf8PathBuf, ProjectManifest)>> {
    let manifest_path = match explicit_manifest {
        Some(path) => resolve_relative(repo_root, path),
        None => match crate::discovery::manifest(repo_root)? {
            crate::discovery::ManifestDiscovery::Found(manifest) => manifest.path,
            crate::discovery::ManifestDiscovery::NotFound => return Ok(None),
            crate::discovery::ManifestDiscovery::Conflict { found } => {
                let paths = found
                    .iter()
                    .map(|manifest| manifest.path.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return invalid_input(
                    repo_root,
                    format!("multiple project manifests found: {paths}"),
                );
            }
        },
    };
    let encoding = ctx_traits_core::encoding::Encoding::from_path(&manifest_path)?;
    let text = crate::read::read_text(&manifest_path)?;
    let manifest = ctx_traits_core::encoding::decode_manifest(encoding, &text)?;
    Ok(Some((manifest_path, manifest)))
}

struct ResolvedSource {
    manifest_path: Utf8PathBuf,
    source_kind: String,
    source_path: String,
    warnings: Vec<String>,
    /// The resolved Git commit, present only for `git`-sourced dependencies.
    resolved_rev: Option<String>,
}

fn resolve_source(
    repo_root: &Utf8Path,
    declaration: &DependencyDeclaration,
    locked_rev: Option<&str>,
) -> crate::Result<ResolvedSource> {
    let plan = plan_dependency(&declaration.dependency);
    let warnings = plan
        .warnings
        .into_iter()
        .map(|warning| format!("source-plan:{warning:?}"))
        .collect::<Vec<_>>();
    match plan.action {
        SourceAction::ReadLocal { path } => {
            let root = resolve_relative(&declaration.base_dir, Utf8Path::new(&path));
            let manifest_path = package_manifest_path(&root)?;
            Ok(ResolvedSource {
                manifest_path,
                source_kind: "local-path".to_string(),
                source_path: root.to_string(),
                warnings,
                resolved_rev: None,
            })
        }
        SourceAction::ReadNpm {
            package,
            package_path,
        } => {
            // Shares its entire fetch/resolve/integrity/extraction/
            // discovery/claim-verification pipeline with project installs
            // (`ctx traits install`) via `crate::distribution::stage_npm_package`
            // — the single registry artifact pipeline (P438 recurrence): a
            // package-local npm dependency is refused before vendoring on
            // exactly the same malformed/mismatched `ctx.digests` claim or
            // unsupported schema version that would refuse an install.
            let spec = ctx_traits_core::distribution::parse_spec(&format!(
                "{package}@{}",
                declaration.dependency.version
            ))
            .map_err(ctx_traits_core::Error::from)?;
            let registry = crate::distribution::resolve_registry_options(repo_root);
            let staged = crate::distribution::stage_npm_package(&spec, registry)?;
            let chosen = select_staged_npm_trait(&staged, &package, package_path.as_deref())?;
            let manifest_path = staged.staging_root.join(&chosen.canonical_path);
            Ok(ResolvedSource {
                manifest_path,
                source_kind: "npm".to_string(),
                source_path: format!("{package}@{}", staged.resolved_version),
                warnings,
                resolved_rev: None,
            })
        }
        SourceAction::CloneGit {
            url,
            requested_ref,
            package_path,
        } => {
            let base_dir = git_repo_cache_dir(repo_root, &url)?;
            let legacy_base_dir = legacy_git_repo_cache_dir(repo_root, &url);
            let (rev, checkout_dir) = resolve_git_source(
                &base_dir,
                Some(&legacy_base_dir),
                &url,
                requested_ref.as_deref(),
                locked_rev,
            )?;
            let root = match package_path {
                Some(path) => checkout_dir.join(validate_relative_path(&path)?),
                None => checkout_dir,
            };
            let manifest_path = package_manifest_path(&root)?;
            Ok(ResolvedSource {
                manifest_path,
                source_kind: "git".to_string(),
                source_path: format!("{url}@{rev}"),
                warnings,
                resolved_rev: Some(rev),
            })
        }
        SourceAction::Unsupported { reason } => invalid_input(&declaration.origin, reason),
    }
}

/// Deterministic cache base directory for a Git URL, keyed by a digest of
/// the URL so the URL string cannot influence filesystem layout. Holds a
/// mutable fetch clone (`clone/`) plus one immutable checkout per resolved
/// commit (`rev/<commit>/`) — see [`resolve_git_source`]. Lives under the
/// invocation repository's global per-repository cache root (P426), never
/// under the repo-local `.ctx/cache`.
fn git_repo_cache_dir(repo_root: &Utf8Path, url: &str) -> crate::Result<Utf8PathBuf> {
    let hex = Digest::from_bytes(url.as_bytes())
        .as_str()
        .trim_start_matches("sha256:")
        .to_string();
    let key = crate::state::repo_key(&crate::state::canonical_repo_root(repo_root)?);
    Ok(crate::state::global_cache_root(&key)?.join("git").join(hex))
}

/// One-release legacy fallback base directory for [`git_repo_cache_dir`]'s
/// pre-P426 repo-local shape (`.ctx/cache/git/<hex>`). Never written to —
/// only [`resolve_git_source`]'s locked-revision fast path reads an already
/// materialized immutable checkout here.
fn legacy_git_repo_cache_dir(repo_root: &Utf8Path, url: &str) -> Utf8PathBuf {
    let hex = Digest::from_bytes(url.as_bytes())
        .as_str()
        .trim_start_matches("sha256:")
        .to_string();
    repo_root.join(".ctx").join("cache").join("git").join(hex)
}

fn git_clone_dir(base_dir: &Utf8Path) -> Utf8PathBuf {
    base_dir.join("clone")
}

fn git_rev_checkout_dir(base_dir: &Utf8Path, rev: &str) -> Utf8PathBuf {
    base_dir.join("rev").join(rev)
}

/// Resolve a Git dependency source to `(commit, checkout_dir)`.
///
/// `checkout_dir` is an immutable, revision-keyed directory distinct from the
/// mutable fetch clone: two syncs resolving different refs of the same URL
/// each get their own checkout directory, so one can never read content
/// belonging to the other's resolved revision. When `locked_rev` already has
/// a materialized checkout under `base_dir` (global) or, failing that,
/// `legacy_base_dir` (the one-release pre-P426 repo-local fallback, read-only
/// — a hit there is never copied or written back), it is reused directly with
/// no network access. Otherwise the fetch, resolution, and checkout run under
/// an exclusive per-URL `flock` held for the whole sequence, so concurrent
/// syncs of the same URL serialize instead of racing the shared mutable
/// clone; a contender that arrives after the lock is released simply finds
/// its checkout directory already published and reuses it.
fn resolve_git_source(
    base_dir: &Utf8Path,
    legacy_base_dir: Option<&Utf8Path>,
    url: &str,
    requested_ref: Option<&str>,
    locked_rev: Option<&str>,
) -> crate::Result<(String, Utf8PathBuf)> {
    if let Some(rev) = locked_rev {
        if !is_canonical_git_rev(rev) {
            return Err(crate::git_process::message_error(format!(
                "locked git revision {rev:?} is not a full 40- or 64-character commit hash"
            )));
        }
        let checkout_dir = git_rev_checkout_dir(base_dir, rev);
        if checkout_dir.is_dir() {
            return Ok((rev.to_string(), checkout_dir));
        }
        if let Some(legacy_base_dir) = legacy_base_dir {
            let legacy_checkout_dir = git_rev_checkout_dir(legacy_base_dir, rev);
            if legacy_checkout_dir.is_dir() {
                return Ok((rev.to_string(), legacy_checkout_dir));
            }
        }
    }

    std::fs::create_dir_all(base_dir).map_err(|source| crate::environment::Error::Filesystem {
        path: base_dir.to_string(),
        source,
    })?;
    let lock_path = base_dir.join("fetch.lock");
    let lock_file = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    crate::file_lock::lock_exclusive_blocking(&lock_file).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    // `lock_file`'s exclusive flock releases on drop at function return,
    // covering every path below including the re-check and early returns.

    if let Some(rev) = locked_rev {
        let checkout_dir = git_rev_checkout_dir(base_dir, rev);
        if checkout_dir.is_dir() {
            return Ok((rev.to_string(), checkout_dir));
        }
    }

    let clone_dir = git_clone_dir(base_dir);
    ensure_git_clone(&clone_dir, url)?;
    let rev = match locked_rev {
        Some(rev) => {
            let resolved = resolve_full_commit(&clone_dir, rev)?;
            if resolved != rev {
                return Err(crate::git_process::message_error(format!(
                    "locked git revision {rev:?} does not resolve to itself (git reports {resolved:?}); locked revisions must already be the full commit id"
                )));
            }
            resolved
        }
        None => resolve_git_rev(&clone_dir, requested_ref)?,
    };
    let checkout_dir = git_rev_checkout_dir(base_dir, &rev);
    if !checkout_dir.is_dir() {
        checkout_immutable_rev(&clone_dir, &checkout_dir, &rev)?;
    }
    Ok((rev, checkout_dir))
}

/// Clone `url` into `clone_dir` if absent, otherwise fetch. A first clone is
/// staged into a sibling temporary directory and published via rename only
/// after it succeeds, so a failed clone never leaves a directory that later
/// appears valid. Callers hold [`resolve_git_source`]'s per-URL lock across
/// this call, so no other process can observe or race the staging/rename.
fn ensure_git_clone(clone_dir: &Utf8Path, url: &str) -> crate::Result<()> {
    if clone_dir.join(".git").is_dir() {
        let output = run_git_long(clone_dir, &["fetch", "--force", "--tags", "origin"])?;
        if !output.success {
            return Err(git_error("git fetch origin", &output));
        }
        return Ok(());
    }
    let parent = clone_dir
        .parent()
        .expect("git clone dir always has a parent under .ctx/cache/git/<hash>")
        .to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|source| crate::environment::Error::Filesystem {
        path: parent.to_string(),
        source,
    })?;
    let staging = parent.join(format!(
        "{}.partial-{}",
        clone_dir.file_name().unwrap_or("clone"),
        std::process::id()
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: staging.to_string(),
                source,
            }
        })?;
    }
    let clone_result = run_git_long(
        &parent,
        &[
            "clone",
            "--no-checkout",
            "--origin",
            "origin",
            url,
            staging.as_str(),
        ],
    );
    match clone_result {
        Ok(output) if output.success => {}
        Ok(output) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(git_error("git clone", &output));
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    }
    std::fs::rename(&staging, clone_dir).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: clone_dir.to_string(),
            source,
        }
    })?;
    Ok(())
}

/// `true` when `rev` is a full, canonical Git object id: 40 hex characters
/// (SHA-1) or 64 (SHA-256) — matching the offline-pinned check core's
/// `source_plan::is_offline_pinned_ref` applies to a requested ref.
fn is_canonical_git_rev(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.chars().all(|c| c.is_ascii_hexdigit())
}

/// Resolve `spec` to its full commit id via `git rev-parse --verify`,
/// rejecting any result that is not itself a canonical full-length hex id.
fn resolve_full_commit(exec_dir: &Utf8Path, spec: &str) -> crate::Result<String> {
    let full_spec = format!("{spec}^{{commit}}");
    let output = run_git(exec_dir, &["rev-parse", "--verify", &full_spec])?;
    if !output.success {
        return Err(git_error(
            &format!("git rev-parse --verify {spec}"),
            &output,
        ));
    }
    let resolved = output.stdout.trim().to_string();
    if !is_canonical_git_rev(&resolved) {
        return Err(git_error(
            &format!("git rev-parse --verify {spec}"),
            &output,
        ));
    }
    Ok(resolved)
}

/// Resolve `requested_ref` (or the remote default when absent) to a full
/// commit ID, preferring the fetched `origin/<ref>` form so floating
/// branches resolve to their latest fetched tip.
fn resolve_git_rev(clone_dir: &Utf8Path, requested_ref: Option<&str>) -> crate::Result<String> {
    let candidates: Vec<String> = match requested_ref {
        Some(r) => vec![format!("origin/{r}"), r.to_string()],
        None => vec!["origin/HEAD".to_string(), "HEAD".to_string()],
    };
    let mut last_output = None;
    for candidate in &candidates {
        let spec = format!("{candidate}^{{commit}}");
        let output = run_git(clone_dir, &["rev-parse", "--verify", &spec])?;
        if output.success {
            let rev = output.stdout.trim().to_string();
            if is_canonical_git_rev(&rev) {
                return Ok(rev);
            }
        }
        last_output = Some(output);
    }
    Err(git_error(
        &format!("git rev-parse ref {requested_ref:?}"),
        &last_output.expect("at least one ref candidate is always tried"),
    ))
}

/// Materialize `rev`'s full tree into `checkout_dir`, an immutable directory
/// distinct from `clone_dir`'s own (checkout-less) working tree. Uses
/// `--work-tree`/`--git-dir` rather than `git worktree add` so no worktree
/// registration is left behind in the shared clone. Checkout happens into a
/// private staging directory beside `checkout_dir` and is published via
/// `rename` only after it succeeds, so a crash or interruption mid-checkout
/// never leaves a directory at `checkout_dir` that a later run — or the
/// lock-free fast path in [`resolve_git_source`] — could mistake for a
/// complete, verified tree. Callers hold `resolve_git_source`'s per-URL
/// lock, so this never races another process for `clone_dir`'s index file.
fn checkout_immutable_rev(
    clone_dir: &Utf8Path,
    checkout_dir: &Utf8Path,
    rev: &str,
) -> crate::Result<()> {
    let parent = checkout_dir
        .parent()
        .expect("checkout dir always has a parent under .ctx/cache/git/<hash>/rev")
        .to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|source| crate::environment::Error::Filesystem {
        path: parent.to_string(),
        source,
    })?;
    let staging = parent.join(format!(
        "{}.partial-{}",
        checkout_dir.file_name().unwrap_or("rev"),
        std::process::id()
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: staging.to_string(),
                source,
            }
        })?;
    }
    std::fs::create_dir_all(&staging).map_err(|source| crate::environment::Error::Filesystem {
        path: staging.to_string(),
        source,
    })?;
    let git_dir = clone_dir.join(".git");
    let output = run_git(
        clone_dir,
        &[
            "--git-dir",
            git_dir.as_str(),
            "--work-tree",
            staging.as_str(),
            "checkout",
            "--force",
            rev,
            "--",
            ".",
        ],
    );
    match output {
        Ok(output) if output.success => {}
        Ok(output) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(git_error("git checkout", &output));
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    }
    std::fs::rename(&staging, checkout_dir).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: checkout_dir.to_string(),
            source,
        }
    })?;
    Ok(())
}

/// Vendored-dependency capture ceiling, matching the worktree module's
/// listing-capture convention rather than the former silent 65,536-byte cap.
const DEPENDENCY_CAPTURE_LIMIT: usize = 16 * 1024 * 1024;

/// Short/bounded Git plumbing calls (`rev-parse`, `checkout`) that never
/// legitimately run long.
fn run_git(exec_dir: &Utf8Path, args: &[&str]) -> crate::Result<crate::command::RunOutput> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(exec_dir),
        cwd: None,
        args,
        success_exit_code: &[0],
        timeout_ms: crate::git_process::PLUMBING_TIMEOUT_MS,
        capture_limit: DEPENDENCY_CAPTURE_LIMIT,
    })?;
    output.refuse_if_truncated(&format!("git {}", args.join(" ")))?;
    Ok(output)
}

/// `clone`/`fetch`: duration depends on remote network and repository size,
/// not a fixed local plumbing cost, so these get the long timeout — resolved
/// fresh via `[git] long-seconds` (P489) rather than the fixed constant, so a
/// declared override applies here exactly like it does for `rebase`/
/// `worktree add`.
fn run_git_long(exec_dir: &Utf8Path, args: &[&str]) -> crate::Result<crate::command::RunOutput> {
    let output = crate::git_process::run(crate::git_process::Request {
        exec_dir: Some(exec_dir),
        cwd: None,
        args,
        success_exit_code: &[0],
        timeout_ms: crate::harness_config::resolve_git_long_timeout_ms(Utf8Path::new(".")),
        capture_limit: DEPENDENCY_CAPTURE_LIMIT,
    })?;
    output.refuse_if_truncated(&format!("git {}", args.join(" ")))?;
    Ok(output)
}

fn git_error(command: &str, output: &crate::command::RunOutput) -> crate::Error {
    crate::git_process::error(command, output)
}

fn verify_locked_dependency(
    repo_root: &Utf8Path,
    lockfile: &crate::lockfile::Document,
    dependency: &Dependency,
    loaded: &LoadedDependency,
    vendored_root: &Utf8Path,
    resolved_rev: Option<&str>,
) -> crate::Result<(String, Vec<String>)> {
    let mut warnings = Vec::new();
    let Some(entry) = lockfile.dependency_entry(&dependency.alias) else {
        return Ok(("lock-missing".to_string(), warnings));
    };
    if let Some(actual_rev) = resolved_rev {
        match entry.rev.as_deref() {
            Some(locked_rev) if locked_rev == actual_rev => {}
            Some(locked_rev) => warnings.push(format!(
                "git revision differs from lock: expected {locked_rev}, actual {actual_rev}"
            )),
            None => warnings.push("missing locked git revision".to_string()),
        }
    }
    compare_digest(
        &mut warnings,
        "source",
        entry.source_digest(),
        loaded.source_digest.as_str(),
    );
    compare_digest(
        &mut warnings,
        "canonical",
        entry.canonical_digest(),
        loaded.canonical_digest.as_str(),
    );
    compare_digest(
        &mut warnings,
        "model-visible",
        entry.model_visible_digest(),
        loaded.model_visible_digest.as_str(),
    );
    compare_digest(
        &mut warnings,
        "resource-manifest",
        entry.resource_manifest_digest(),
        loaded.resource_manifest_digest.as_str(),
    );
    let vendored_manifest =
        crate::layout::vendored_dependency_manifest(repo_root, &dependency.alias)?;
    if !vendored_manifest.exists() {
        warnings.push(format!("vendored package missing at {vendored_root}"));
    } else if !package_subset_matches(&loaded.root, vendored_root)? {
        warnings.push("vendored package differs from resolved source".to_string());
    }
    let status = if warnings.is_empty() {
        "locked"
    } else {
        "drift"
    };
    Ok((status.to_string(), warnings))
}

fn compare_digest(warnings: &mut Vec<String>, label: &str, expected: Option<&str>, actual: &str) {
    match expected {
        Some(expected) if expected == actual => {}
        Some(expected) => warnings.push(format!(
            "{label} digest differs from lock: expected {expected}, actual {actual}"
        )),
        None => warnings.push(format!("missing locked {label} digest")),
    }
}

fn lock_entry_for(
    dependency: &Dependency,
    loaded: &LoadedDependency,
    vendored_path: &str,
    rev: Option<String>,
) -> crate::Result<crate::lockfile::LockDependencyEntry> {
    Ok(crate::lockfile::LockDependencyEntry {
        alias: dependency.alias.clone(),
        id: dependency.id.clone(),
        requested_version: Some(dependency.version.clone()),
        resolved_version: Some(loaded.version.clone()),
        source: dependency
            .source
            .as_ref()
            .map(toml::Value::try_from)
            .transpose()
            .map_err(|source| crate::parse::Error::TomlEncode {
                context: format!("encode lock source for dependency {}", dependency.alias),
                source,
            })?,
        vendored_path: vendored_path.to_string(),
        digests: Some(crate::lockfile::LockDependencyDigests {
            source: Some(loaded.source_digest.as_str().to_string()),
            canonical: Some(loaded.canonical_digest.as_str().to_string()),
            model_visible: Some(loaded.model_visible_digest.as_str().to_string()),
            resource_manifest: Some(loaded.resource_manifest_digest.as_str().to_string()),
        }),
        rev,
        attestation: None,
    })
}

fn copy_package_subset(source_root: &Utf8Path, vendored_root: &Utf8Path) -> crate::Result<bool> {
    let changed = !package_subset_matches(source_root, vendored_root)?;
    if !changed {
        return Ok(false);
    }
    if vendored_root.exists() {
        std::fs::remove_dir_all(vendored_root).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: vendored_root.to_string(),
                source,
            }
        })?;
    }
    std::fs::create_dir_all(vendored_root).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: vendored_root.to_string(),
            source,
        }
    })?;
    for (relative, source_path) in package_subset_files(source_root)? {
        let target_path = vendored_root.join(&relative);
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                crate::environment::Error::Filesystem {
                    path: parent.to_string(),
                    source,
                }
            })?;
        }
        std::fs::copy(&source_path, &target_path).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: target_path.to_string(),
                source,
            }
        })?;
    }
    Ok(true)
}

fn package_subset_matches(source_root: &Utf8Path, vendored_root: &Utf8Path) -> crate::Result<bool> {
    if !vendored_root.exists() {
        return Ok(false);
    }
    let source_files = package_subset_files(source_root)?;
    let vendor_files = package_subset_files(vendored_root)?;
    if source_files.keys().ne(vendor_files.keys()) {
        return Ok(false);
    }
    for (relative, source_path) in source_files {
        let source_bytes = std::fs::read(&source_path).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: source_path.to_string(),
                source,
            }
        })?;
        let Some(vendor_path) = vendor_files.get(&relative) else {
            return Ok(false);
        };
        let vendor_bytes =
            std::fs::read(vendor_path).map_err(|source| crate::environment::Error::Filesystem {
                path: vendor_path.to_string(),
                source,
            })?;
        if source_bytes != vendor_bytes {
            return Ok(false);
        }
    }
    Ok(true)
}

fn package_subset_files(root: &Utf8Path) -> crate::Result<BTreeMap<Utf8PathBuf, Utf8PathBuf>> {
    let mut files = BTreeMap::new();
    if let Some(manifest) = crate::layout::resolve_package_manifest(root) {
        files.insert(Utf8PathBuf::from(crate::layout::PACKAGE_MANIFEST), manifest);
    }
    for dir in ["reference", "resources"] {
        let path = root.join(dir);
        if path.exists() {
            let mut relative_files = BTreeSet::new();
            collect_regular_files(root, &path, &mut relative_files)?;
            files.extend(
                relative_files
                    .into_iter()
                    .map(|relative| (relative.clone(), root.join(relative))),
            );
        }
    }
    Ok(files)
}

fn collect_regular_files(
    root: &Utf8Path,
    dir: &Utf8Path,
    files: &mut BTreeSet<Utf8PathBuf>,
) -> crate::Result<()> {
    let meta =
        std::fs::symlink_metadata(dir).map_err(|source| crate::environment::Error::Filesystem {
            path: dir.to_string(),
            source,
        })?;
    if meta.file_type().is_symlink() {
        return invalid_input(
            dir,
            "dependency package contains a symlink in vendored subset",
        );
    }
    if !meta.file_type().is_dir() {
        return invalid_input(dir, "dependency package subset path is not a directory");
    }
    let mut children = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|source| crate::environment::Error::Filesystem {
        path: dir.to_string(),
        source,
    })? {
        let entry = entry.map_err(|source| crate::environment::Error::Filesystem {
            path: dir.to_string(),
            source,
        })?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            crate::environment::Error::Filesystem {
                path: path.display().to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "dependency path is not UTF-8",
                ),
            }
        })?;
        children.push(path);
    }
    children.sort();
    for path in children {
        let meta = std::fs::symlink_metadata(&path).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            }
        })?;
        if meta.file_type().is_symlink() {
            return invalid_input(
                &path,
                "dependency package contains a symlink in vendored subset",
            );
        }
        if meta.file_type().is_dir() {
            collect_regular_files(root, &path, files)?;
        } else if meta.file_type().is_file() {
            let relative =
                path.strip_prefix(root)
                    .map_err(|_| crate::environment::Error::Filesystem {
                        path: path.to_string(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "dependency package path escapes root",
                        ),
                    })?;
            files.insert(relative.to_path_buf());
        }
    }
    Ok(())
}

fn package_manifest_path(root_or_file: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let manifest = if root_or_file.extension().is_some() {
        root_or_file.to_path_buf()
    } else {
        crate::layout::resolve_package_manifest(root_or_file).unwrap_or_else(|| {
            root_or_file
                .join(crate::layout::GENERATED)
                .join(crate::layout::CANONICAL_MANIFEST)
        })
    };
    if !manifest.is_file() {
        return invalid_input(&manifest, "dependency package does not contain trait.toml");
    }
    Ok(manifest)
}

/// Select which trait inside a staged (possibly multi-trait) npm package one
/// package-local `source.package` npm dependency resolves to.
///
/// `package_path`, when given, must name the exact package-root-relative
/// path of one discovered trait package — the same explicit-path convention
/// project install/`info` capability reporting uses. This is an exact
/// identity match against each trait's own discovered package root, never a
/// prefix search over `canonical_path`, so a parent path matching several
/// staged traits or a non-package descendant/ancestor path both fail
/// explicitly instead of silently selecting one. With no `package_path`,
/// exactly one discovered trait is required: an npm package staging more
/// than one trait is ambiguous for a dependency declaration that names only
/// one npm package, and must disambiguate with an explicit path.
fn select_staged_npm_trait<'a>(
    staged: &'a crate::distribution::StagedPackage,
    package: &str,
    package_path: Option<&str>,
) -> crate::Result<&'a crate::distribution::StagedTrait> {
    match package_path {
        Some(path) => {
            let relative = validate_relative_path(path)?;
            let relative = relative.as_str().trim_matches('/');
            let mut matches = staged.traits.iter().filter(|t| t.package_root == relative);
            let found = matches.next();
            match (found, matches.next()) {
                (Some(found), None) => Ok(found),
                (Some(_), Some(_)) => invalid_input(
                    package,
                    format!(
                        "npm package {package} has multiple trait packages under {path:?}; name the exact package root"
                    ),
                ),
                (None, _) => invalid_input(
                    package,
                    format!("npm package {package} has no trait package at {path:?}"),
                ),
            }
        }
        None => match staged.traits.as_slice() {
            [only] => Ok(only),
            _ => invalid_input(
                package,
                format!(
                    "npm package {package} contains {} trait packages; set source.package.path to select one",
                    staged.traits.len()
                ),
            ),
        },
    }
}

fn validate_relative_path(path: &str) -> crate::Result<Utf8PathBuf> {
    if path.trim().is_empty() || path.contains('\\') || path.contains("//") {
        return invalid_input(path, "dependency source path has an unsafe shape");
    }
    let candidate = Utf8Path::new(path);
    if candidate.is_absolute() {
        return invalid_input(path, "dependency source path must be relative");
    }
    let mut normalized = Utf8PathBuf::new();
    for component in candidate.components() {
        match component {
            Utf8Component::Normal(part) => normalized.push(part),
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir | Utf8Component::RootDir | Utf8Component::Prefix(_) => {
                return invalid_input(path, "dependency source path must not escape its root");
            }
        }
    }
    Ok(normalized)
}

fn resolve_relative(base: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    // Fold `.`/`..` lexically: downstream package-root derivation and
    // vendored-subset enumeration match on literal ancestor names, which a
    // remaining `..` component defeats.
    let mut normalized = Utf8PathBuf::new();
    for component in joined.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir
                if matches!(
                    normalized.components().next_back(),
                    Some(Utf8Component::Normal(_))
                ) =>
            {
                normalized.pop();
            }
            _ => normalized.push(component),
        }
    }
    normalized
}

fn resource_read_warning_strings(warnings: &[crate::resource::ResourceReadWarning]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| match warning {
            crate::resource::ResourceReadWarning::SymlinkDetected { resource_id, path } => {
                format!("symlink-detected:{resource_id}:{path}")
            }
            crate::resource::ResourceReadWarning::MissingFile { resource_id, path } => {
                format!("missing-file:{resource_id}:{path}")
            }
            crate::resource::ResourceReadWarning::SpecialFile { resource_id, path } => {
                format!("special-file:{resource_id}:{path}")
            }
            crate::resource::ResourceReadWarning::Directory { resource_id, path } => {
                format!("directory:{resource_id}:{path}")
            }
            crate::resource::ResourceReadWarning::BinaryContent {
                resource_id,
                path,
                byte_size,
            } => format!("binary-content:{resource_id}:{path}:{byte_size}"),
        })
        .collect()
}

fn invalid_input<T>(path: impl std::fmt::Display, message: impl Into<String>) -> crate::Result<T> {
    Err(crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into()),
    }
    .into())
}
