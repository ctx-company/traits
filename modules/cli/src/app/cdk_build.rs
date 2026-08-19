//! CLI orchestration for CDK-backed trait builds.

use camino::{Utf8Path, Utf8PathBuf};

pub(crate) struct CdkSynthOutcome {
    pub(crate) source_path: Utf8PathBuf,
    pub(crate) run: ctx_traits_io::cdk_build::CdkBuildOutcome,
    pub(crate) response: ctx_traits_core::synth::Response,
    pub(crate) source_map: ctx_traits_core::source_map::SourceMap,
    pub(crate) authoring_diagnostics: Vec<CdkAuthoringDiagnostic>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct CdkAuthoringDiagnostic {
    pub(crate) severity: String,
    pub(crate) code: String,
    #[serde(rename = "fieldPath")]
    pub(crate) field_path: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct CdkAuthoredDeclaration {
    kind: String,
    #[serde(rename = "ref")]
    reference: String,
    declaration: serde_json::Value,
    source: Option<ctx_traits_core::source_map::SourceAnchor>,
}

#[derive(serde::Deserialize)]
struct CdkBuildEnvelope {
    draft: serde_json::Value,
    #[serde(default, rename = "__map")]
    source_map: ctx_traits_core::source_map::SourceMap,
    #[serde(default)]
    diagnostics: Vec<CdkAuthoringDiagnostic>,
    #[serde(default, rename = "authoredDeclarations")]
    authored_declarations: Vec<CdkAuthoredDeclaration>,
    #[serde(default, rename = "traitDependencies")]
    trait_dependencies: Vec<CdkTraitDependency>,
}

/// One dependency trait package the import-graph walk crossed into during a
/// single build (task 0170). `files` are the `file:` URLs of that package's
/// own files that were actually loaded — the exact input set the derived
/// `[[dependency]].digest` is computed over.
#[derive(Debug, Clone, serde::Deserialize)]
struct CdkTraitDependency {
    #[serde(rename = "packageRoot")]
    package_root: String,
    #[serde(default, rename = "packageJsonName")]
    package_json_name: Option<String>,
    #[serde(default, rename = "packageJsonVersion")]
    package_json_version: Option<String>,
    files: Vec<String>,
}

/// One resolved variant emitted inside a `FamilyEnvelope` (`packages/cdk/src/variant.ts`).
#[derive(Debug, Clone, serde::Deserialize)]
struct CdkFamilyVariantEnvelope {
    path: String,
    draft: serde_json::Value,
    #[serde(default, rename = "sourceMap")]
    source_map: ctx_traits_core::source_map::SourceMap,
    #[serde(default)]
    diagnostics: Vec<CdkAuthoringDiagnostic>,
}

/// The tagged family envelope `resolveTraitFamily` emits: `{ family: true,
/// id, version, topology, variants }`. Distinguished from [`CdkBuildEnvelope`]
/// structurally, by the presence of the `family` key rather than `draft`.
///
/// `#[serde(alias = "leaves")]` accepts an older `@ctx-traits/cdk` package
/// that still emits the pre-rename `leaves` key, so a new binary keeps
/// building against an outdated npm dependency (skew the other way — a new
/// npm package against an old binary — is unhandled pre-v1).
#[derive(Debug, Clone, serde::Deserialize)]
struct CdkFamilyBuildEnvelope {
    id: String,
    version: String,
    topology: serde_json::Value,
    #[serde(alias = "leaves")]
    variants: Vec<CdkFamilyVariantEnvelope>,
    /// Trait dependency packages the shared import-graph walk crossed into
    /// while resolving the family module (task 0170) — the resolve tracker
    /// runs once per Node process, so this list applies to every variant.
    #[serde(default, rename = "traitDependencies")]
    trait_dependencies: Vec<CdkTraitDependency>,
}

/// One family variant's complete synth result, keyed by its variant name
/// (e.g. `"quick"`). Consumed by [`publish_cdk_family`] to write the
/// variant's canonical output and refresh the package's `[family]` manifest
/// table.
pub(crate) struct CdkFamilyVariantOutcome {
    pub(crate) name: String,
    pub(crate) response: ctx_traits_core::synth::Response,
    pub(crate) source_map: ctx_traits_core::source_map::SourceMap,
}

/// Complete synth result for a native trait family (`trait(id, {
/// variants })`): one [`CdkFamilyVariantOutcome`] per resolved variant, plus
/// the shared family identity and topology needed to write the package
/// manifest's `[family]` table.
pub(crate) struct CdkFamilySynthOutcome {
    pub(crate) family_id: String,
    pub(crate) family_version: String,
    pub(crate) topology: serde_json::Value,
    pub(crate) variants: Vec<CdkFamilyVariantOutcome>,
}

/// After a single-trait build writes canonical output, regenerate the
/// package-root `package.json` from `trait.toml` — build owns it end to
/// end, so hand edits never survive a rebuild. A no-op for a build target
/// that isn't a recognized package root (e.g. a bare CDK source with no
/// package manifest yet), same shape as [`publish_cdk_family`]'s filter.
pub(crate) fn write_generated_package_json_if_packaged(
    source_path: &Utf8Path,
) -> crate::Result<()> {
    let Some(package_root) =
        ctx_traits_io::layout::package_root_for_manifest(source_path).filter(|root| {
            ctx_traits_io::layout::is_canonical_package_root(root)
                || ctx_traits_io::layout::is_builtin_template_package_root(root)
        })
    else {
        return Ok(());
    };
    let manifest_path = ctx_traits_io::layout::package_manifest_path(package_root);
    if !manifest_path.is_file() {
        return Ok(());
    }
    let manifest_text = ctx_traits_io::read::read_text(&manifest_path)?;
    let Some(manifest) =
        ctx_traits_core::manifest::decode_package_manifest(&manifest_text, manifest_path.as_str())?
    else {
        return Ok(());
    };
    Ok(ctx_traits_io::cdk_build::write_generated_package_json(
        package_root,
        &manifest,
    )?)
}

pub(crate) fn family_variant_aliases(family_id: &str, name: &str) -> Vec<String> {
    vec![format!("{family_id}-{name}")]
}

/// After a successful publish, make `generated/` contain EXACTLY the new
/// topology's artifacts — `generated/` is machine-owned (0179: location
/// states authority), so anything stale in it is the previous format's
/// leftover, not authored content. A family publish
/// (`keep_variants = Some(names)`) prunes variant directories the source no
/// longer declares plus any single-era top-level `index.*` canonical; a
/// single-trait publish (`keep_variants = None`) prunes every variant
/// directory. Without this, a package migrating between the family and
/// single-trait shapes (either direction) leaves an orphaned canonical
/// beside the real one, and resolution/locks trip over whichever they find
/// first. Runs only after every new artifact is written — a failed build
/// never prunes.
pub(crate) fn reconcile_generated_layout(
    generated: &Utf8Path,
    keep_variants: Option<&[String]>,
) -> crate::Result<()> {
    let entries = match std::fs::read_dir(generated.as_std_path()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(crate::Error::from(ctx_traits_io::Error::from(
                ctx_traits_io::environment::Error::Filesystem {
                    path: generated.to_string(),
                    source,
                },
            )));
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = entry.path();
        let remove: bool = if path.is_dir() {
            match keep_variants {
                Some(names) => !names.iter().any(|kept| kept == name),
                None => true,
            }
        } else {
            // Single-era canonical artifacts at the generated/ top level are
            // stale exactly when the package now publishes variants.
            keep_variants.is_some()
                && matches!(
                    name,
                    "index.toml" | "index.json" | "index.yaml" | "index.map"
                )
        };
        if !remove {
            continue;
        }
        let outcome = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(source) = outcome {
            return Err(crate::Error::from(ctx_traits_io::Error::from(
                ctx_traits_io::environment::Error::Filesystem {
                    path: generated.join(name).to_string(),
                    source,
                },
            )));
        }
    }
    Ok(())
}

/// Either shape a CDK build source can emit: an ordinary single-trait draft,
/// or a native family's resolved variants.
pub(crate) enum CdkSynthResult {
    Single(Box<CdkSynthOutcome>),
    Family(Box<CdkFamilySynthOutcome>),
}

pub(crate) fn synthesize_cdk_source(
    source_path: &Utf8Path,
    output_format: ctx_traits_core::synth::OutputFormat,
) -> crate::Result<CdkSynthOutcome> {
    match synthesize_cdk_source_any(source_path, output_format)? {
        CdkSynthResult::Single(outcome) => Ok(*outcome),
        CdkSynthResult::Family(family) => Err(crate::Error::Command {
            message: format!(
                "{source_path} declares a native trait family ({} variants under \"{}\"); \
                 this command does not yet build native families",
                family.variants.len(),
                family.family_id
            ),
        }),
    }
}

/// Build a CDK source, accepting either an ordinary single-trait draft or a
/// native family's resolved variants. See [`synthesize_cdk_source`] for the
/// single-trait-only entry point existing callers use.
pub(crate) fn synthesize_cdk_source_any(
    source_path: &Utf8Path,
    output_format: ctx_traits_core::synth::OutputFormat,
) -> crate::Result<CdkSynthResult> {
    let source_path = source_path.to_path_buf();
    let repo_root = stable_repo_root(&source_path)?;
    let run =
        ctx_traits_io::cdk_build::emit_draft_json(ctx_traits_io::cdk_build::CdkBuildRequest {
            source_path: source_path.clone(),
            repo_root: Some(repo_root.clone()),
            timeout_ms: ctx_traits_io::cdk_build::DEFAULT_BUILD_TIMEOUT_MS,
            capture_limit: ctx_traits_io::harness::DEFAULT_CAPTURE_LIMIT,
            env: Vec::new(),
        })?;
    let emitted_json: serde_json::Value = serde_json::from_str(&run.stdout).map_err(|source| {
        crate::Error::json(
            format!("CDK module emitted non-JSON on stdout from {source_path}"),
            source,
        )
    })?;
    if is_family_envelope(&emitted_json) {
        let family = synthesize_cdk_family(&source_path, &repo_root, emitted_json, output_format)?;
        return Ok(CdkSynthResult::Family(Box::new(family)));
    }
    let (
        draft_json,
        source_map,
        mut authoring_diagnostics,
        authored_declarations,
        trait_dependencies,
    ) = parse_cdk_build_stdout(emitted_json)?;
    let draft_json = stamp_derived_dependencies(draft_json, &trait_dependencies)?;
    // The CDK runtime captures each construct's source anchor from a Node
    // `Error().stack` frame, which is inherently an OS-absolute (or
    // `file://`-decoded absolute) path — there is no "relative mode" to ask
    // Node for. Repo-relativize here, the one place a `repo_root` value is
    // already available, so the emitted map is reproducible across machines
    // and worktrees (a committed `generated/index.map` must not embed a
    // developer's local checkout path).
    let source_map = relativize_source_map(source_map, &repo_root)?;
    authoring_diagnostics.extend(orphan_diagnostics(
        &source_path,
        &authored_declarations,
        &source_map,
    )?);
    authoring_diagnostics.extend(structural_lint_diagnostics(&source_path)?);
    let mut provenance_warnings = Vec::new();
    provenance_warnings.extend(
        authoring_diagnostics
            .iter()
            .map(format_authoring_diagnostic),
    );
    let response = ctx_traits_core::synth::synthesize(ctx_traits_core::synth::Request {
        document_kind: ctx_traits_core::synth::DocumentKind::Trait,
        draft_json,
        output_format,
        provenance: ctx_traits_core::synth::ProvenanceSeed {
            generator_package: Some("ctx-traits-cli-build".to_string()),
            generator_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            source_path: Some(source_path.to_string()),
            warnings: provenance_warnings,
        },
    })?;

    Ok(CdkSynthResult::Single(Box::new(CdkSynthOutcome {
        source_path,
        run,
        response,
        source_map,
        authoring_diagnostics,
    })))
}

/// Structural family-envelope detection: `{ "family": true, ... }`, matching
/// `isTraitFamilyHandle`'s tag in `packages/cdk/src/variant.ts` rather than
/// attempting a full deserialize first (a malformed single-trait envelope
/// must not be misreported as "not a family").
fn is_family_envelope(emitted_json: &serde_json::Value) -> bool {
    emitted_json
        .as_object()
        .and_then(|object| object.get("family"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Parse a family envelope's variants and synthesize each one through the
/// same pure synth path a single-trait build uses, relativizing each
/// variant's source map against the shared `repo_root`.
fn synthesize_cdk_family(
    source_path: &Utf8Path,
    repo_root: &Utf8Path,
    emitted_json: serde_json::Value,
    output_format: ctx_traits_core::synth::OutputFormat,
) -> crate::Result<CdkFamilySynthOutcome> {
    let envelope: CdkFamilyBuildEnvelope = serde_json::from_value(emitted_json)
        .map_err(|source| crate::Error::json("decode CDK family build envelope", source))?;
    let mut variants = Vec::with_capacity(envelope.variants.len());
    for variant in envelope.variants {
        ctx_traits_core::source_map::validate_source_map(&variant.source_map)?;
        let source_map = relativize_source_map(variant.source_map, repo_root)?;
        let provenance_warnings = variant
            .diagnostics
            .iter()
            .map(format_authoring_diagnostic)
            .collect::<Vec<_>>();
        let draft = stamp_derived_dependencies(variant.draft, &envelope.trait_dependencies)?;
        let response = ctx_traits_core::synth::synthesize(ctx_traits_core::synth::Request {
            document_kind: ctx_traits_core::synth::DocumentKind::Trait,
            draft_json: draft,
            output_format,
            provenance: ctx_traits_core::synth::ProvenanceSeed {
                generator_package: Some("ctx-traits-cli-build".to_string()),
                generator_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                source_path: Some(format!("{source_path}#{}", variant.path)),
                warnings: provenance_warnings,
            },
        })?;
        variants.push(CdkFamilyVariantOutcome {
            name: variant.path,
            response,
            source_map,
        });
    }
    Ok(CdkFamilySynthOutcome {
        family_id: envelope.id,
        family_version: envelope.version,
        topology: envelope.topology,
        variants,
    })
}

/// One family variant's written build output, used to assemble the
/// `[family]` manifest table.
pub(crate) struct CdkFamilyVariantBuildEvidence {
    pub(crate) name: String,
    pub(crate) target_path: Utf8PathBuf,
    pub(crate) map_path: Utf8PathBuf,
    pub(crate) aliases: Vec<String>,
}

/// Complete evidence for one `ctx traits build` run over a native family
/// source: every variant's written output plus the package manifest now
/// carrying the `[family]` table.
pub(crate) struct CdkFamilyBuildEvidence {
    pub(crate) family_id: String,
    pub(crate) family_version: String,
    pub(crate) manifest_path: Utf8PathBuf,
    pub(crate) variants: Vec<CdkFamilyVariantBuildEvidence>,
}

/// Publish a resolved native family: write each variant's canonical output
/// and source map under `generated/<name>/`, then write/refresh the
/// package's `[family]` manifest table so resolution (`run.rs`) and
/// per-variant run-config can find every variant by its name and legacy
/// aliases (`<family-id>-<name>`, skipped for the default variant, which
/// already resolves via `family.default`).
pub(crate) fn publish_cdk_family(
    source_path: &Utf8Path,
    output_format: ctx_traits_core::synth::OutputFormat,
    family: &CdkFamilySynthOutcome,
) -> crate::Result<CdkFamilyBuildEvidence> {
    let package_root = ctx_traits_io::layout::package_root_for_manifest(source_path)
        .filter(|root| {
            ctx_traits_io::layout::is_canonical_package_root(root)
                || ctx_traits_io::layout::is_builtin_template_package_root(root)
        })
        .ok_or_else(|| crate::Error::Command {
            message: format!(
                "{source_path} declares a native trait family but is not under a recognized \
                 package's source/index.{{ts,mjs}} root; native families are only supported for \
                 canonical or template packages"
            ),
        })?
        .to_path_buf();
    let default_name = family
        .topology
        .as_object()
        .and_then(|topology| topology.get("default"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| crate::Error::Command {
            message: format!(
                "{source_path}: family topology is missing its top-level \"default\" name"
            ),
        })?
        .to_string();
    validate_family_manifest_identity(&package_root, &family.family_id, &family.family_version)?;
    let generated = package_root.join(ctx_traits_io::layout::GENERATED);
    let mut variants = Vec::with_capacity(family.variants.len());
    for variant in &family.variants {
        let ctx_traits_core::synth::CanonicalDocument::Trait(built) = &variant.response.canonical
        else {
            return Err(crate::Error::Command {
                message: format!(
                    "family variant {:?} did not synthesize a trait document",
                    variant.name
                ),
            });
        };
        if built.id.as_str() != family.family_id {
            return Err(crate::Error::Command {
                message: format!(
                    "family variant {:?} built id {:?} does not match family id {:?}",
                    variant.name,
                    built.id.as_str(),
                    family.family_id
                ),
            });
        }
        if built.variant.as_deref() != Some(variant.name.as_str()) {
            return Err(crate::Error::Command {
                message: format!(
                    "family variant {:?} built variant {:?} does not match its own name",
                    variant.name, built.variant
                ),
            });
        }
        let variant_dir = generated.join(&variant.name);
        let target_path = variant_dir.join(format!("index.{}", output_format.extension()));
        let map_path = variant_dir.join(ctx_traits_io::layout::CANONICAL_SOURCE_MAP);
        ctx_traits_io::write::write_build_output(&target_path, &variant.response.output_text)?;
        let map_json = serde_json::to_string_pretty(&variant.source_map)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|e| crate::Error::json("serialize CDK family variant source map", e))?;
        ctx_traits_io::write::write_build_output(&map_path, &map_json)?;
        // Keep every historical hyphenated name, including the default
        // variant. A bare family id remains the preferred default name.
        let aliases = family_variant_aliases(&family.family_id, &variant.name);
        variants.push(CdkFamilyVariantBuildEvidence {
            name: variant.name.clone(),
            target_path,
            map_path,
            aliases,
        });
    }
    let variant_names = variants
        .iter()
        .map(|variant| variant.name.clone())
        .collect::<Vec<_>>();
    reconcile_generated_layout(&generated, Some(&variant_names))?;
    let manifest_path = ctx_traits_io::layout::package_manifest_path(&package_root);
    let manifest_entries = variants
        .iter()
        .map(
            |variant| ctx_traits_io::family_manifest::FamilyVariantManifestEntry {
                name: variant.name.clone(),
                relative_path: variant
                    .target_path
                    .strip_prefix(&package_root)
                    .unwrap_or(&variant.target_path)
                    .to_string(),
                aliases: variant.aliases.clone(),
            },
        )
        .collect::<Vec<_>>();
    ctx_traits_io::family_manifest::write_family_table(
        &manifest_path,
        &default_name,
        &manifest_entries,
    )?;
    write_generated_package_json_if_packaged(source_path)?;
    Ok(CdkFamilyBuildEvidence {
        family_id: family.family_id.clone(),
        family_version: family.family_version.clone(),
        manifest_path,
        variants,
    })
}

/// Enforce the same `[package]` identity invariant [`build_cdk_package`]'s
/// single-trait path enforces (`validate_package_manifest_identity` in
/// `schema_synth_build.rs`), before a single variant is written: a native
/// family package must already have a root `trait.toml` with a `[package]`
/// table whose `id`/`version` match the family's declared id/version. A
/// missing manifest, a manifest with no `[package]` table, or a mismatched
/// id/version is refused — never silently created or overwritten, since
/// [`ctx_traits_io::family_manifest::write_family_table`] would otherwise
/// happily extend an empty document into an invalid package manifest that
/// carries `[family]` but no `[package]`.
fn validate_family_manifest_identity(
    package_root: &Utf8Path,
    family_id: &str,
    family_version: &str,
) -> crate::Result<()> {
    let manifest_path = ctx_traits_io::layout::package_manifest_path(package_root);
    if !manifest_path.is_file() {
        return Err(crate::Error::Command {
            message: format!(
                "{manifest_path} does not exist; a native trait family requires an existing \
                 package manifest with a [package] table declaring id {family_id:?} and \
                 version {family_version:?} before it can be built"
            ),
        });
    }
    let manifest_text = ctx_traits_io::read::read_text(&manifest_path)?;
    let manifest =
        ctx_traits_core::manifest::decode_package_manifest(&manifest_text, manifest_path.as_str())?
            .ok_or_else(|| crate::Error::Command {
                message: format!(
                    "{manifest_path} has no [package] table; a native trait family requires an \
                     existing package manifest declaring id {family_id:?} and version \
                     {family_version:?} before it can be built"
                ),
            })?;
    if manifest.package.id != family_id || manifest.package.version != family_version {
        return Err(crate::Error::Command {
            message: format!(
                "package manifest {manifest_path} declares [package] {}@{} but the built family \
                 is {family_id}@{family_version}; align trait source and manifest",
                manifest.package.id, manifest.package.version,
            ),
        });
    }
    Ok(())
}

/// Select the one stable base a build's source map is written (and later
/// read back) relative to: the Git worktree root containing `source_path`
/// when there is one, so a construct declared in a sibling nested workspace
/// package (e.g. `packages/agents/src/index.ts` referenced from a
/// `packages/rust/.ctx/traits/*` build) still lands under the same root as
/// the package being built, rather than a package-local root that can't
/// reach it. Falls back to the existing trait/package-root inference for
/// fresh non-Git projects (e.g. the golden-path fixtures), which have no
/// Git worktree to discover. Propagates a genuine Git operational failure
/// (a timed-out invocation, an unexpected exit code, `git` failing to spawn)
/// as an error rather than silently falling back to package-root inference,
/// which could otherwise select too narrow a root and emit a machine-specific
/// absolute path into a committed map.
pub(crate) fn stable_repo_root(source_path: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let probe_dir = source_path.parent().unwrap_or(source_path);
    match ctx_traits_io::repository::discover_repo_root_at(probe_dir)? {
        Some(root) => Ok(root),
        None => {
            Ok(ctx_traits_io::export::infer_repo_root_from_trait_file(source_path).to_path_buf())
        }
    }
}

/// Rewrite every anchor's `file` to be relative to `repo_root`, using a
/// stable forward-slash separator so the emitted bytes don't depend on the
/// building platform. Every generated anchor must fall under `repo_root` —
/// ordinary in-repo constructs (including nested workspace packages, since
/// `repo_root` is the containing Git worktree, not the package root) always
/// do. An anchor outside `repo_root` cannot be made repository-relative
/// without corrupting source-map semantics, so it is a controlled build
/// error instead of a silently retained absolute, machine-specific path.
fn relativize_source_map(
    source_map: ctx_traits_core::source_map::SourceMap,
    repo_root: &Utf8Path,
) -> crate::Result<ctx_traits_core::source_map::SourceMap> {
    // `repo_root` may be relative (even empty, e.g. when the invoked
    // package root's repo root is the cwd itself) — absolutize it purely
    // by string manipulation first (works even if the path can't be
    // `canonicalize`d, e.g. it doesn't exist). The Node stack frames the
    // CDK runtime captures anchors from are always absolute *and*
    // symlink-resolved (e.g. macOS's `/tmp` -> `/private/tmp`), so a plain
    // string-prefix strip against the absolutized-but-unresolved
    // `repo_root` can still fail to match when the source was reached
    // through a symlinked path (e.g. a `mktemp -d` result); resolve
    // symlinks on both sides too before stripping, falling back to the
    // unresolved form if either path doesn't (yet) exist on disk.
    let repo_root = normalized_absolute_path(repo_root)?;
    let canonical_repo_root = canonicalize_utf8(&repo_root).unwrap_or(repo_root);
    source_map
        .into_iter()
        .map(|(reference, mut anchor)| {
            let file_path = Utf8Path::new(&anchor.file);
            let canonical_file =
                canonicalize_utf8(file_path).unwrap_or_else(|| file_path.to_path_buf());
            let relative = canonical_file.strip_prefix(&canonical_repo_root).map_err(|_| {
                crate::Error::Command {
                    message: format!(
                        "source anchor {canonical_file} falls outside repository root {canonical_repo_root} and cannot be made repository-relative"
                    ),
                }
            })?;
            anchor.file = stable_separators(relative);
            Ok((reference, anchor))
        })
        .collect()
}

/// Render a relative path with a stable forward-slash separator regardless
/// of the building platform, so the emitted map bytes are byte-identical
/// across Unix and Windows checkouts.
fn stable_separators(path: &Utf8Path) -> String {
    path.components()
        .map(|component| component.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Inverse of [`relativize_source_map`]: join every relative anchor `file`
/// onto `repo_root` so a consumer (critique/explain evidence) gets a real
/// filesystem path back, using the same [`stable_repo_root`] base the
/// writer used. An anchor that is already absolute passes through
/// unchanged — committed maps predating this normalization, or a map read
/// during the writer→reader base migration itself, must not be corrupted by
/// a second join.
pub(crate) fn rebase_source_map(
    source_map: ctx_traits_core::source_map::SourceMap,
    repo_root: &Utf8Path,
) -> ctx_traits_core::source_map::SourceMap {
    source_map
        .into_iter()
        .map(|(reference, mut anchor)| {
            let file_path = Utf8Path::new(&anchor.file);
            if file_path.is_relative() {
                anchor.file = repo_root.join(file_path).into_string();
            }
            (reference, anchor)
        })
        .collect()
}

fn canonicalize_utf8(path: &Utf8Path) -> Option<Utf8PathBuf> {
    std::fs::canonicalize(path)
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
}

#[allow(clippy::type_complexity)]
fn parse_cdk_build_stdout(
    emitted_json: serde_json::Value,
) -> crate::Result<(
    serde_json::Value,
    ctx_traits_core::source_map::SourceMap,
    Vec<CdkAuthoringDiagnostic>,
    Vec<CdkAuthoredDeclaration>,
    Vec<CdkTraitDependency>,
)> {
    let Some(object) = emitted_json.as_object() else {
        return Ok((
            emitted_json,
            ctx_traits_core::source_map::SourceMap::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
    };
    if !object.contains_key("draft") {
        return Ok((
            emitted_json,
            ctx_traits_core::source_map::SourceMap::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
    }
    let envelope: CdkBuildEnvelope =
        serde_json::from_value(serde_json::Value::Object(object.clone()))
            .map_err(|source| crate::Error::json("decode CDK build envelope", source))?;
    ctx_traits_core::source_map::validate_source_map(&envelope.source_map)?;
    Ok((
        envelope.draft,
        envelope.source_map,
        envelope.diagnostics,
        envelope.authored_declarations,
        envelope.trait_dependencies,
    ))
}

/// Derive an alias slug from an npm package name for a build-stamped
/// dependency row: the scope-less final path segment, lowercased, with any
/// character outside `[a-z0-9-]` collapsed to `-`. Collisions across scopes
/// (`@a/x` and `@b/x`) are caught by [`ctx_traits_core::manifest::validate_dependencies`]'s
/// duplicate-alias check, same as a hand-authored collision would be.
fn derive_dependency_alias(package_json_name: Option<&str>, package_root: &Utf8Path) -> String {
    let raw = package_json_name
        .and_then(|name| name.rsplit('/').next())
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| package_root.file_name().unwrap_or("dependency").to_string());
    let slug: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "dependency".to_string()
    } else {
        slug.to_string()
    }
}

/// Digest of the dependency content actually consumed: sorted
/// (package-root-relative-path, content bytes) pairs of every file the
/// import-graph walk recorded as loaded from that package, so the stamp is
/// insensitive to files the dependency ships but this build never imported.
fn digest_trait_dependency_files(
    package_root: &Utf8Path,
    file_urls: &[String],
) -> crate::Result<ctx_traits_core::digest::Digest> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(file_urls.len());
    for url in file_urls {
        let path = url_to_path(url)?;
        let contents =
            std::fs::read(path.as_std_path()).map_err(|source| crate::Error::Command {
                message: format!("reading dependency file {path}: {source}"),
            })?;
        let relative = path
            .strip_prefix(package_root)
            .map(|rel| rel.as_str().to_string())
            .unwrap_or_else(|_| path.to_string());
        entries.push((relative, contents));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut buffer = Vec::new();
    for (relative, contents) in entries {
        buffer.extend_from_slice(relative.as_bytes());
        buffer.push(0);
        buffer.extend_from_slice(&contents);
        buffer.push(0);
    }
    Ok(ctx_traits_core::digest::Digest::from_bytes(&buffer))
}

fn url_to_path(url: &str) -> crate::Result<Utf8PathBuf> {
    let rest = url
        .strip_prefix("file://")
        .ok_or_else(|| crate::Error::Command {
            message: format!("expected a file: URL from the CDK resolve tracker, got {url:?}"),
        })?;
    let decoded = percent_decode(rest);
    Utf8PathBuf::from_path_buf(std::path::PathBuf::from(decoded)).map_err(|path| {
        crate::Error::Command {
            message: format!("resolve-tracker file URL is not valid UTF-8: {path:?}"),
        }
    })
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&input[i + 1..i + 3], 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Stamp derived `[[dependency]]` rows into a draft's `dependency` array for
/// every trait package the import-graph walk crossed into during this build
/// (task 0170). Nothing here is user-authored: id/version come from the
/// dependency's own `trait.toml`, digest is computed from the exact files
/// this build loaded from it. Returns the draft unchanged when no trait
/// dependency was crossed.
fn stamp_derived_dependencies(
    mut draft_json: serde_json::Value,
    trait_dependencies: &[CdkTraitDependency],
) -> crate::Result<serde_json::Value> {
    if trait_dependencies.is_empty() {
        return Ok(draft_json);
    }
    let mut stamped = Vec::with_capacity(trait_dependencies.len());
    for dep in trait_dependencies {
        let package_root = Utf8PathBuf::from(&dep.package_root);
        let manifest_path = ctx_traits_io::layout::package_manifest_path(&package_root);
        let manifest_text = ctx_traits_io::read::read_text(&manifest_path)?;
        let manifest = ctx_traits_core::manifest::decode_package_manifest(
            &manifest_text,
            manifest_path.as_str(),
        )?
        .ok_or_else(|| crate::Error::Command {
            message: format!(
                "dependency trait package at {package_root} has no decodable trait.toml"
            ),
        })?;
        // Consumer-side dependency health gate (task 0170 Watch): a
        // dependency trait package with no `trait.lock` of its own has no
        // pinned resolution to trust, so the CONSUMER build refuses naming
        // it rather than silently stamping unpinned content.
        if !ctx_traits_io::lockfile::lockfile_path(&package_root).exists() {
            return Err(crate::Error::Command {
                message: format!(
                    "dependency {:?} has no trait.lock at {package_root}; a dependency trait \
                     package must ship a lock recording its own pinned resolution before it \
                     can be consumed",
                    manifest.package.id
                ),
            });
        }
        // The dependency's own `trait.lock` records a self entry (written by
        // `ctx_traits_io::dependency::sync`'s "self" load, which every
        // `ctx traits build` performs via `record_lock_evidence`, regardless
        // of whether the package declares dependencies of its own) whose
        // `digests.source` pins that trait's own canonical content. Re-verify
        // it in place with the same VerifyLocked path `ctx traits check
        // --locked` uses, rather than re-deriving a second digest scheme: a
        // dependency whose shipped canonical no longer matches its own
        // shipped lock (tampered in transit, or shipped stale) must not be
        // silently trusted just because a lock file happens to exist. Only
        // runs when the dependency ships a decodable canonical trait
        // document — packages that expose plain importable source with no
        // canonical of their own have nothing here to self-verify against.
        // `resolve_package_manifest`'s third candidate is the flat root
        // `trait.toml` used by legacy vendored packages, which would
        // wrongly resolve back to the v2 package manifest we just decoded
        // above (a v2 package's root `trait.toml` is never its canonical) —
        // so only the two `generated/` candidates apply here.
        let canonical_candidates = [
            package_root.join("generated").join("index.toml"),
            package_root.join("generated").join("trait.toml"),
        ];
        if let Some(canonical_path) = canonical_candidates
            .into_iter()
            .find(|candidate| candidate.is_file())
        {
            let self_check =
                ctx_traits_io::dependency::sync(ctx_traits_io::dependency::SyncRequest {
                    repo_root: &package_root,
                    manifest_path: None,
                    trait_file: Some(&canonical_path),
                    mode: ctx_traits_io::dependency::SyncMode::VerifyLocked,
                    vendor_root_override: None,
                })?;
            if !self_check.passed {
                return Err(crate::Error::Command {
                    message: format!(
                        "dependency {:?} at {package_root} has a trait.lock that does not match \
                         its shipped content ({}); a tampered or stale dependency package cannot \
                         be consumed",
                        manifest.package.id,
                        self_check.warnings.join("; ")
                    ),
                });
            }
        }
        let digest = digest_trait_dependency_files(&package_root, &dep.files)?;
        let source = if package_root
            .components()
            .any(|component| component.as_str() == "node_modules")
        {
            ctx_traits_core::manifest::TraitSource::Npm {
                package: dep
                    .package_json_name
                    .clone()
                    .unwrap_or_else(|| manifest.package.id.clone()),
                package_path: None,
            }
        } else {
            ctx_traits_core::manifest::TraitSource::local(package_root.to_string())
        };
        stamped.push(ctx_traits_core::manifest::Dependency {
            alias: derive_dependency_alias(dep.package_json_name.as_deref(), &package_root),
            id: manifest.package.id.clone(),
            version: dep
                .package_json_version
                .clone()
                .unwrap_or_else(|| manifest.package.version.clone()),
            source: Some(source),
            digest: Some(digest.as_str().to_string()),
        });
    }
    // The CDK draft can already carry hand-authored `[[dependency]]` rows via
    // the legacy qualified-ref interop path (`packages/cdk/src/trait.ts`
    // `dependency?: CanonicalDependency | readonly CanonicalDependency[]`),
    // which the task keeps working, just untaught. Stamped rows are appended
    // to whatever is already declared, never replace it — an authored trait
    // that also imports a dependency package must keep both.
    let mut combined: Vec<ctx_traits_core::manifest::Dependency> = draft_json
        .as_object()
        .and_then(|object| object.get("dependency"))
        .cloned()
        .map(|value| match value {
            serde_json::Value::Array(_) => serde_json::from_value(value),
            other => serde_json::from_value(serde_json::Value::Array(vec![other])),
        })
        .transpose()
        .map_err(|source| crate::Error::json("decode authored dependency rows", source))?
        .unwrap_or_default();
    combined.extend(stamped);
    combined.sort_by(|a, b| a.id.cmp(&b.id));
    ctx_traits_core::manifest::validate_dependencies(&combined)?;
    let combined_json = serde_json::to_value(&combined)
        .map_err(|source| crate::Error::json("encode derived dependency stamps", source))?;
    if let Some(object) = draft_json.as_object_mut() {
        object.insert("dependency".to_string(), combined_json);
    }
    Ok(draft_json)
}

fn orphan_diagnostics(
    source_path: &Utf8Path,
    declarations: &[CdkAuthoredDeclaration],
    source_map: &ctx_traits_core::source_map::SourceMap,
) -> crate::Result<Vec<CdkAuthoringDiagnostic>> {
    // `source_path` is the authored entry module, so its parent is the one
    // package source root used by the IO import boundary. Keep this
    // representation identical for filtering and anchor enrichment.
    let source_root = ctx_traits_io::cdk_build::source_root_for_entry(source_path);
    let source_root = canonicalize_utf8(source_root).unwrap_or_else(|| source_root.to_path_buf());
    let diagnostics = declarations
        .iter()
        .filter_map(|declaration| {
            if !matches!(declaration.kind.as_str(), "slot" | "schema")
                || source_map.contains_key(&declaration.reference)
            {
                return None;
            }
            let source = declaration.source.as_ref()?;
            let file = Utf8Path::new(&source.file);
            let canonical_file = canonicalize_utf8(file).unwrap_or_else(|| file.to_path_buf());
            if canonical_file.strip_prefix(&source_root).is_err() {
                return None;
            }
            let id = declaration
                .declaration
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&declaration.reference);
            let binding = std::fs::read_to_string(file).ok().and_then(|text| {
                text.lines()
                    .nth(source.start.saturating_sub(1))
                    .and_then(|line| {
                        let marker = line.find("const ")?;
                        let rest = &line[marker + 6..];
                        let end = rest.find(|character: char| {
                            !character.is_ascii_alphanumeric()
                                && character != '_'
                                && character != '$'
                        })?;
                        Some(rest[..end].to_string())
                    })
            });
            Some(CdkAuthoringDiagnostic {
                severity: "warning".to_string(),
                code: "cdk-orphan-declaration".to_string(),
                field_path: declaration.reference.clone(),
                message: format!(
                "authored {} {}{} is unreachable and absent from emitted canonical output ({}:{})",
                declaration.kind,
                id,
                binding.map_or_else(String::new, |name| format!(" ({name})")),
                source.file,
                source.start,
            ),
            })
        })
        .collect::<Vec<_>>();
    Ok(diagnostics)
}

/// Surface the P533 mechanical structural lints (`cdk-index-defines`,
/// `cdk-generic-module-name`, `cdk-inline-prompt-body`) through the same
/// `CdkAuthoringDiagnostic` shape as `cdk-orphan-declaration`, WARN-only, no
/// new rendering path.
fn structural_lint_diagnostics(
    source_path: &Utf8Path,
) -> crate::Result<Vec<CdkAuthoringDiagnostic>> {
    let source_root = ctx_traits_io::cdk_build::source_root_for_entry(source_path);
    let source_root = canonicalize_utf8(source_root).unwrap_or_else(|| source_root.to_path_buf());
    Ok(
        ctx_traits_io::cdk_build::collect_structural_lints(source_path)?
            .into_iter()
            .map(|lint| CdkAuthoringDiagnostic {
                severity: "warning".to_string(),
                code: lint.code.to_string(),
                field_path: lint
                    .file
                    .strip_prefix(&source_root)
                    .map(|relative| relative.to_string())
                    .unwrap_or_else(|_| lint.file.to_string()),
                message: format!("{}: {}", lint.file, lint.message),
            })
            .collect(),
    )
}

pub(crate) fn format_authoring_diagnostic(diagnostic: &CdkAuthoringDiagnostic) -> String {
    format!(
        "{} [{}] {}: {}",
        diagnostic.severity, diagnostic.code, diagnostic.field_path, diagnostic.message
    )
}

pub(crate) fn is_import_boundary_error(error: &crate::Error) -> bool {
    error.to_string().contains("resolves outside source root")
}

pub(crate) fn default_target_path(
    source_path: &Utf8Path,
    output_format: ctx_traits_core::synth::OutputFormat,
) -> Utf8PathBuf {
    source_path.with_extension(output_format.extension())
}

/// Resolve independent protocol output and authoring sidecar paths for a CDK
/// source.
///
/// Canonical (`.ctx/traits/<id>`) and P271 template
/// (`modules/builtin/templates/<id>`) packages route to the
/// `generated/index.{ext,map}` pair. Every other package sharing the v2
/// `<root>/source/index.{ts,mjs}` shape — including the seven first-party
/// built-in meta-traits under `modules/builtin/traits`, which still
/// commit an adjacent `source/index.map` — keeps the adjacent-sidecar
/// fallback. Widening this to the built-in meta-trait tree too is a
/// separate, larger migration (it touches seven committed sidecars) and is
/// out of scope for P271.
pub(crate) fn package_build_paths(
    source_path: &Utf8Path,
    output_format: ctx_traits_core::synth::OutputFormat,
    out: Option<&str>,
) -> crate::Result<(Utf8PathBuf, Utf8PathBuf)> {
    let fallback = |source_path: &Utf8Path| {
        (
            out.map_or_else(
                || default_target_path(source_path, output_format),
                Utf8PathBuf::from,
            ),
            source_path.with_extension("map"),
        )
    };
    if !matches!(source_path.file_name(), Some("index.ts" | "index.mjs")) {
        // Legacy root `trait.ts`/`trait.mjs` authoring keeps the adjacent
        // sidecar convention.
        return Ok(fallback(source_path));
    }
    let Some(source_dir) = source_path.parent() else {
        return Ok(fallback(source_path));
    };
    if source_dir.file_name() != Some(ctx_traits_io::layout::SOURCE_DIR) {
        return Ok(fallback(source_path));
    }
    let Some(package_root) = ctx_traits_io::layout::package_root_for_manifest(source_path) else {
        return Ok(fallback(source_path));
    };
    // `package_root_for_manifest` only steps up past `source/` to the
    // package root proper when the grandparent is a recognized canonical
    // (`.ctx/traits`), built-in/template, or built-in-store root;
    // otherwise it returns the `source/` directory itself unchanged,
    // meaning this isn't a recognized v2 package root.
    if package_root == source_dir {
        return Ok(fallback(source_path));
    }
    let is_canonical = ctx_traits_io::layout::is_canonical_package_root(package_root);
    let is_template = ctx_traits_io::layout::is_builtin_template_package_root(package_root);
    if !is_canonical && !is_template {
        return Ok(fallback(source_path));
    }
    let generated = package_root.join(ctx_traits_io::layout::GENERATED);
    let protocol_output = generated.join(format!("index.{}", output_format.extension()));
    let target = out.map_or(protocol_output, Utf8PathBuf::from);
    let map = generated.join(ctx_traits_io::layout::CANONICAL_SOURCE_MAP);
    Ok((target, map))
}

/// Reject build paths that would overwrite either the source or source map.
pub(crate) fn ensure_distinct_build_paths(
    source_path: &Utf8Path,
    target_path: &Utf8Path,
    map_path: &Utf8Path,
) -> crate::Result<()> {
    let paths = [source_path, target_path, map_path];
    let normalized = paths
        .iter()
        .map(|path| normalized_absolute_path(path))
        .collect::<crate::Result<Vec<_>>>()?;
    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        if normalized[left] == normalized[right] || resolved_paths_match(paths[left], paths[right])?
        {
            return Err(crate::Error::Command {
                message: format!(
                    "CDK build source, output, and source map must be distinct: {} conflicts with {}",
                    paths[left], paths[right]
                ),
            });
        }
    }
    Ok(())
}

fn normalized_absolute_path(path: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let current_dir = std::env::current_dir().map_err(|source| crate::Error::Command {
            message: format!("resolve current directory for CDK build paths: {source}"),
        })?;
        let current_dir =
            Utf8PathBuf::from_path_buf(current_dir).map_err(|path| crate::Error::Command {
                message: format!("current directory is not UTF-8: {}", path.display()),
            })?;
        current_dir.join(path)
    };
    let mut normalized = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            camino::Utf8Component::Prefix(prefix) => normalized.push(prefix.as_str()),
            camino::Utf8Component::RootDir => normalized.push("/"),
            camino::Utf8Component::CurDir => {}
            camino::Utf8Component::ParentDir => {
                normalized.pop();
            }
            camino::Utf8Component::Normal(component) => normalized.push(component),
        }
    }
    Ok(normalized)
}

fn resolved_paths_match(left: &Utf8Path, right: &Utf8Path) -> crate::Result<bool> {
    let left = match std::fs::canonicalize(left) {
        Ok(path) => Utf8PathBuf::from_path_buf(path).map_err(|path| crate::Error::Command {
            message: format!("resolved CDK build path is not UTF-8: {}", path.display()),
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(crate::Error::Command {
                message: format!("resolve CDK build path {left}: {source}"),
            });
        }
    };
    let right = match std::fs::canonicalize(right) {
        Ok(path) => Utf8PathBuf::from_path_buf(path).map_err(|path| crate::Error::Command {
            message: format!("resolved CDK build path is not UTF-8: {}", path.display()),
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(crate::Error::Command {
                message: format!("resolve CDK build path {right}: {source}"),
            });
        }
    };
    Ok(left == right)
}

pub(crate) fn package_cdk_source(trait_path: &Utf8Path) -> crate::Result<Option<Utf8PathBuf>> {
    let Some(package_root) = ctx_traits_io::layout::package_root_for_manifest(trait_path) else {
        return Ok(None);
    };
    let repo_root = ctx_traits_io::export::infer_repo_root_from_trait_file(trait_path);
    let protocol_root = ctx_traits_io::layout::trait_protocol_root_path(repo_root);
    if package_root.parent() == Some(protocol_root.as_path()) {
        // A genuine `.ctx/traits/<id>` package: resolve through the
        // authoring-root-keyed lookup (covers both the v2 `source/index.*`
        // shape and the legacy root `trait.ts`/`trait.mjs` shape).
        let Some(trait_id) = package_root.file_name() else {
            return Ok(None);
        };
        return ctx_traits_io::layout::trait_cdk_source_path(repo_root, trait_id)
            .map_err(ctx_traits_io::Error::from)
            .map_err(crate::Error::from);
    }
    // Any other package root — a first-party built-in or template package
    // under `modules/builtin/{traits,templates}`, or an external flat
    // package — resolves the CDK source relative to the package root that
    // was actually resolved for `trait_path`. Reconstructing a path from
    // the trait id under `.ctx/traits` here (as this used to) can silently
    // resolve an unrelated, same-named live `.ctx/traits/<id>` package
    // instead of the package actually being checked (e.g. the
    // `modules/builtin/templates/review/` template colliding with
    // this repository's own live `.ctx/traits/authored/review/`), or find
    // nothing at all for ids with no live counterpart.
    ctx_traits_io::layout::package_cdk_source_path(package_root)
        .map_err(ctx_traits_io::Error::from)
        .map_err(crate::Error::from)
}

/// Resolve the canonical source map for repository packages while preserving
/// adjacent sidecars for external flat packages.
pub(crate) fn package_source_map(trait_path: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let source_path = package_cdk_source(trait_path)?;
    let Some(package_root) = ctx_traits_io::layout::package_root_for_manifest(trait_path) else {
        return Ok(trait_path.with_extension("map"));
    };
    let repo_root = ctx_traits_io::export::infer_repo_root_from_trait_file(trait_path);
    let protocol_root = ctx_traits_io::layout::trait_protocol_root_path(repo_root);
    if package_root.parent() == Some(protocol_root.as_path()) {
        if let Some(variant_name) = trait_path
            .parent()
            .filter(|parent| {
                parent.parent().is_some_and(|generated| {
                    generated.file_name() == Some(ctx_traits_io::layout::GENERATED)
                })
            })
            .and_then(Utf8Path::file_name)
        {
            return Ok(package_root
                .join(ctx_traits_io::layout::GENERATED)
                .join(variant_name)
                .join(ctx_traits_io::layout::CANONICAL_SOURCE_MAP));
        }
        let Some(trait_id) = package_root.file_name() else {
            return Ok(trait_path.with_extension("map"));
        };
        return ctx_traits_io::layout::trait_source_map_path(repo_root, trait_id)
            .map_err(ctx_traits_io::Error::from)
            .map_err(crate::Error::from);
    }
    // A first-party P271 template package under
    // `modules/builtin/templates` authoring at the v2
    // `source/index.{ts,mjs}` shape: same canonical `generated/index.map`
    // sidecar `ctx traits build` writes for it (see `package_build_paths`).
    // The built-in meta-trait tree under `modules/builtin/traits`
    // shares the same v2 shape but still commits an adjacent
    // `source/index.map`, so it is deliberately excluded here — see
    // `package_build_paths`'s doc comment.
    let is_template_v2_shape = source_path.as_ref().is_some_and(|source| {
        source.parent().and_then(Utf8Path::file_name) == Some(ctx_traits_io::layout::SOURCE_DIR)
            && source.parent().and_then(Utf8Path::parent) == Some(package_root)
            && ctx_traits_io::layout::is_builtin_template_package_root(package_root)
    });
    if is_template_v2_shape {
        return Ok(package_root
            .join(ctx_traits_io::layout::GENERATED)
            .join(ctx_traits_io::layout::CANONICAL_SOURCE_MAP));
    }
    Ok(source_path.map_or_else(
        || trait_path.with_extension("map"),
        |source| source.with_extension("map"),
    ))
}

/// Preserve the adjacent-source convention for external packages.
fn adjacent_cdk_source(
    package_root: &Utf8Path,
    trait_id: &str,
) -> crate::Result<Option<Utf8PathBuf>> {
    let ts = package_root.join("trait.ts");
    let mjs = package_root.join("trait.mjs");
    match (ts.is_file(), mjs.is_file()) {
        (true, true) => Err(ctx_traits_io::Error::from(
            ctx_traits_io::layout::Error::ConflictingCdkSources {
                id: trait_id.to_string(),
            },
        )
        .into()),
        (true, false) => Ok(Some(ts)),
        (false, true) => Ok(Some(mjs)),
        (false, false) => Ok(None),
    }
}

/// Reject ambiguous explicit CDK package sources before executing either one.
pub(crate) fn ensure_unambiguous_cdk_source(source_path: &Utf8Path) -> crate::Result<()> {
    // v2 entry `<pkg>/source/index.{ts,mjs}`: the package root is one level
    // above `source/`; conflict-check against a leftover legacy root source.
    if matches!(
        source_path.file_name(),
        Some("index.ts") | Some("index.mjs")
    ) && source_path.parent().and_then(Utf8Path::file_name)
        == Some(ctx_traits_io::layout::SOURCE_DIR)
    {
        let Some(package_root) = source_path.parent().and_then(Utf8Path::parent) else {
            return Ok(());
        };
        let Some(trait_id) = package_root.file_name() else {
            return Ok(());
        };
        let sibling = if source_path.file_name() == Some("index.ts") {
            source_path.with_file_name("index.mjs")
        } else {
            source_path.with_file_name("index.ts")
        };
        if sibling.is_file() || adjacent_cdk_source(package_root, trait_id)?.is_some() {
            return Err(ctx_traits_io::Error::from(
                ctx_traits_io::layout::Error::ConflictingCdkSources {
                    id: trait_id.to_string(),
                },
            )
            .into());
        }
        return Ok(());
    }
    let Some(package_root) = source_path.parent() else {
        return Ok(());
    };
    let Some(trait_id) = package_root.file_name() else {
        return Ok(());
    };
    if matches!(
        source_path.file_name(),
        Some("trait.ts") | Some("trait.mjs")
    ) {
        adjacent_cdk_source(package_root, trait_id)?;
        let source_dir = package_root.join(ctx_traits_io::layout::SOURCE_DIR);
        if source_dir.join("index.ts").is_file() || source_dir.join("index.mjs").is_file() {
            return Err(ctx_traits_io::Error::from(
                ctx_traits_io::layout::Error::ConflictingCdkSources {
                    id: trait_id.to_string(),
                },
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod stamp_tests {
    use super::*;

    /// A scratch directory removed on drop, avoiding a `tempfile` dependency
    /// for these three tests.
    struct ScratchDir(Utf8PathBuf);

    impl ScratchDir {
        fn new(label: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = Utf8PathBuf::from_path_buf(std::env::temp_dir())
                .unwrap()
                .join(format!(
                    "ctx-traits-stamp-test-{label}-{}-{n}",
                    std::process::id()
                ));
            std::fs::create_dir_all(path.as_std_path()).unwrap();
            Self(path)
        }

        fn path(&self) -> &Utf8Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.as_std_path());
        }
    }

    /// Writes a minimal trait package (`trait.toml` + one source file) under
    /// `root` and returns the [`CdkTraitDependency`] the resolve tracker
    /// would have recorded for it.
    fn write_dependency_fixture(root: &Utf8Path, id: &str, version: &str) -> CdkTraitDependency {
        std::fs::create_dir_all(root.as_std_path()).unwrap();
        std::fs::write(
            root.join("trait.toml").as_std_path(),
            format!("[package]\nid = \"{id}\"\nversion = \"{version}\"\n"),
        )
        .unwrap();
        let file = root.join("index.mjs");
        std::fs::write(file.as_std_path(), b"export default {};").unwrap();
        std::fs::write(
            root.join("trait.lock").as_std_path(),
            "[metadata]\nschema-version = \"0.1\"\n",
        )
        .unwrap();
        CdkTraitDependency {
            package_root: root.to_string(),
            package_json_name: Some(id.to_string()),
            package_json_version: Some(version.to_string()),
            files: vec![format!("file://{file}")],
        }
    }

    #[test]
    fn stamp_appends_to_authored_dependency_rows() {
        let dir = ScratchDir::new("appends");
        let root = dir.path().join("dep");
        let dep = write_dependency_fixture(&root, "ctx/dep", "0.1.0");

        let authored = serde_json::json!({
            "dependency": {
                "alias": "handauthored",
                "id": "ctx/legacy",
                "version": "1.0.0"
            }
        });

        let stamped = stamp_derived_dependencies(authored, &[dep]).unwrap();
        let rows = stamped
            .get("dependency")
            .and_then(serde_json::Value::as_array)
            .expect("dependency array");
        assert_eq!(
            rows.len(),
            2,
            "authored row must survive alongside the stamp: {rows:?}"
        );
        let ids: Vec<&str> = rows
            .iter()
            .map(|row| row.get("id").and_then(serde_json::Value::as_str).unwrap())
            .collect();
        assert!(ids.contains(&"ctx/legacy"));
        assert!(ids.contains(&"ctx/dep"));
        let stamped_row = rows
            .iter()
            .find(|row| row.get("id").and_then(serde_json::Value::as_str) == Some("ctx/dep"))
            .unwrap();
        assert!(
            stamped_row
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .is_some()
        );
    }

    #[test]
    fn stamp_rejects_alias_collision_between_authored_and_stamped_rows() {
        let dir = ScratchDir::new("collision");
        let root = dir.path().join("dep");
        let dep = write_dependency_fixture(&root, "ctx/dep", "0.1.0");
        // The derived alias for "ctx/dep" is "dep" (scope-less, slugified) —
        // collide it with an authored row using the same alias.
        let authored = serde_json::json!({
            "dependency": {
                "alias": "dep",
                "id": "ctx/other",
                "version": "1.0.0"
            }
        });

        let result = stamp_derived_dependencies(authored, &[dep]);
        assert!(
            result.is_err(),
            "alias collision between authored and stamped rows must be rejected"
        );
    }

    #[test]
    fn stamp_no_dependencies_leaves_draft_unchanged() {
        let draft = serde_json::json!({ "dependency": { "alias": "x", "id": "ctx/x", "version": "1.0.0" } });
        let out = stamp_derived_dependencies(draft.clone(), &[]).unwrap();
        assert_eq!(draft, out);
    }
}
