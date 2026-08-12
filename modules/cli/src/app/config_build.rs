//! CLI orchestration for `ctx traits config build` (0177; retires P457's
//! `RuntimeConfig` target): compile `.ctx/traits/config.ts` into
//! `.ctx/traits/generated/config.toml`, validated against `ConfigDocument`
//! — the committed, declarative project document, not `RuntimeConfig`.
//!
//! This is the one command allowed to bypass the drift guard on its own
//! read path — it never reads the generated document through
//! `ctx_traits_io::config_document::read_document`, only through this
//! module's own node-invoking build, so a drifted artifact never deadlocks
//! the command that repairs it.

use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::response::CommandOutput;

use crate::app::presentation::{
    HumanOutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human,
};

#[derive(serde::Deserialize)]
struct ConfigEmitEnvelope {
    config: serde_json::Value,
    sources: Vec<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConfigBuildReportJson<'a> {
    source: &'a str,
    target: &'a str,
    sources: Vec<&'a str>,
}

/// Resolve the `config.ts` path a `config build [path]` invocation targets:
/// the given path verbatim, or `.ctx/traits/config.ts` relative to the
/// current directory when omitted.
fn resolve_source_path(path: Option<&str>) -> Utf8PathBuf {
    match path {
        Some(path) => Utf8PathBuf::from(path),
        None => Utf8PathBuf::from(ctx_traits_io::layout::CONFIG_SOURCE),
    }
}

/// Is `source_path`'s basename the runtime document source, not the config
/// document source? Dispatches `config build`'s target model/type and
/// output path — see `handle_config_build`.
fn is_runtime_source(source_path: &Utf8Path) -> bool {
    source_path.file_name() == Some("runtime.ts")
}

pub(crate) fn handle_config_build(
    path: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let source_path = resolve_source_path(path);
    let canonical_source = source_path.canonicalize_utf8().map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: source_path.to_string(),
            source,
        })
    })?;
    let repo_root = crate::app::cdk_build::stable_repo_root(&canonical_source)?;
    let config_dir_for_layer = canonical_source
        .parent()
        .unwrap_or_else(|| Utf8Path::new("."))
        .to_path_buf();
    let layer = if ctx_traits_io::harness_config::is_user_global_config_dir(&config_dir_for_layer) {
        "user-global"
    } else {
        "repo"
    };
    if is_runtime_source(&source_path) {
        return handle_runtime_build(&source_path, &canonical_source, &repo_root, layer, json);
    }
    let run = ctx_traits_io::cdk_build::run_node_module(
        ctx_traits_io::cdk_build::CdkBuildRequest {
            source_path: source_path.clone(),
            repo_root: Some(repo_root.clone()),
            timeout_ms: ctx_traits_io::cdk_build::DEFAULT_BUILD_TIMEOUT_MS,
            capture_limit: ctx_traits_io::harness::DEFAULT_CAPTURE_LIMIT,
            env: vec![("CTX_CONFIG_BUILD_LAYER".to_string(), layer.to_string())],
        },
        ctx_traits_io::cdk_build::NODE_EMIT_CONFIG_SCRIPT.as_str(),
        "@ctx-traits/config",
        "config build",
    )?;
    let envelope: ConfigEmitEnvelope = serde_json::from_str(&run.stdout).map_err(|source| {
        crate::Error::json(
            format!("config module emitted non-JSON on stdout from {source_path}"),
            source,
        )
    })?;

    // The TS surface (and therefore the JS object `config.ts` emits) uses
    // the generated camelCase field names; `RuntimeConfig` and the TOML it
    // loads use kebab-case. Rewrite before validating/rendering so both
    // steps see the same shape the loader will.
    let config = crate::app::config_types::camel_to_kebab(&envelope.config)?;

    // Validate through the document's own model: a typo'd key, a bad table
    // shape, or an executable-fact field (structurally absent from
    // `ConfigDocument`) all fail here, in the same `deny_unknown_fields`
    // code that later loads the generated TOML.
    let config_document: ctx_traits_io::config_document::ConfigDocument =
        serde_json::from_value(config.clone()).map_err(|source| {
            crate::Error::json(
                format!("{source_path} does not decode as a valid config"),
                source,
            )
        })?;
    if layer == "user-global" && config_document.vendor.is_some() {
        return Err(ctx_traits_io::Error::Core(
            ctx_traits_core::manifest::Error::InvalidField {
                field_path: "config".to_string(),
                message: format!(
                    "{source_path} declares [vendor] in a global-scope config — dependencies are project identity, not a machine's"
                ),
            }
            .into(),
        )
        .into());
    }

    let config_dir = canonical_source
        .parent()
        .unwrap_or_else(|| Utf8Path::new("."))
        .to_path_buf();
    let mut entries = local_source_entries(&envelope.sources, &config_dir)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let toml_body = render_sorted_toml(&config)?;
    let header = ctx_traits_io::config_source::render_header(&entries);
    let output_text = format!("{header}{toml_body}");

    let target_path =
        ctx_traits_io::layout::trait_generated_root_path(&repo_root).join("config.toml");
    ctx_traits_io::write::write_build_output(&target_path, &output_text)?;

    if json {
        let report = ConfigBuildReportJson {
            source: source_path.as_str(),
            target: target_path.as_str(),
            sources: entries.iter().map(|entry| entry.path.as_str()).collect(),
        };
        crate::app::command_handlers::print_json_report(&report, "config build output")?;
        return Ok(CommandOutput::new(()));
    }

    let panel = config_build_panel(&source_path, &target_path, &entries);
    emit_human(false, &panel, HumanOutputMode::Compact, || Ok(()))?;

    Ok(CommandOutput::new(()))
}

/// `runtime.ts` -> `generated/runtime.toml` half of `config build` (0178):
/// validates the module graph's default export as `RuntimeConfig` (executable
/// facts `ConfigDocument` cannot express decode here) instead of
/// `ConfigDocument`, and mirrors `resolve_runtime_config`'s `[preferences]`/
/// `[repo.*]` global-only refusals at build time so a repo-scope source can
/// never bypass them by never running `config build` against `resolve`.
fn handle_runtime_build(
    source_path: &Utf8Path,
    canonical_source: &Utf8Path,
    repo_root: &Utf8Path,
    layer: &str,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let run = ctx_traits_io::cdk_build::run_node_module(
        ctx_traits_io::cdk_build::CdkBuildRequest {
            source_path: source_path.to_path_buf(),
            repo_root: Some(repo_root.to_path_buf()),
            timeout_ms: ctx_traits_io::cdk_build::DEFAULT_BUILD_TIMEOUT_MS,
            capture_limit: ctx_traits_io::harness::DEFAULT_CAPTURE_LIMIT,
            env: vec![("CTX_CONFIG_BUILD_LAYER".to_string(), layer.to_string())],
        },
        ctx_traits_io::cdk_build::NODE_EMIT_CONFIG_SCRIPT.as_str(),
        "@ctx-traits/config",
        "config build",
    )?;
    let envelope: ConfigEmitEnvelope = serde_json::from_str(&run.stdout).map_err(|source| {
        crate::Error::json(
            format!("runtime module emitted non-JSON on stdout from {source_path}"),
            source,
        )
    })?;
    let config = crate::app::config_types::camel_to_kebab(&envelope.config)?;
    let runtime_config: ctx_traits_io::harness_config::RuntimeConfig =
        serde_json::from_value(config.clone()).map_err(|source| {
            crate::Error::json(
                format!("{source_path} does not decode as a valid runtime config"),
                source,
            )
        })?;
    if layer != "user-global" && !runtime_config.repo.is_empty() {
        return Err(ctx_traits_io::Error::Core(
            ctx_traits_core::manifest::Error::InvalidField {
                field_path: "repo".to_string(),
                message: format!(
                    "{source_path} declares [repo.*] in a repo-scope runtime config — [repo.*] blocks are only accepted in the carried global config file"
                ),
            }
            .into(),
        )
        .into());
    }
    if layer != "user-global" && runtime_config.preferences.is_some() {
        return Err(ctx_traits_io::Error::Core(
            ctx_traits_core::manifest::Error::InvalidField {
                field_path: "preferences".to_string(),
                message: format!(
                    "{source_path} declares [preferences] in a repo-scope runtime config — [preferences] is only accepted in the carried global config file"
                ),
            }
            .into(),
        )
        .into());
    }

    let config_dir = canonical_source
        .parent()
        .unwrap_or_else(|| Utf8Path::new("."))
        .to_path_buf();
    let mut entries = local_source_entries(&envelope.sources, &config_dir)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let toml_body = render_sorted_toml(&config)?;
    let header = ctx_traits_io::config_source::render_header(&entries);
    let output_text = format!("{header}{toml_body}");

    let target_path = if layer == "user-global" {
        config_dir.join("generated").join("runtime.toml")
    } else {
        ctx_traits_io::layout::trait_generated_root_path(repo_root).join("runtime.toml")
    };
    ctx_traits_io::write::write_build_output(&target_path, &output_text)?;

    if json {
        let report = ConfigBuildReportJson {
            source: source_path.as_str(),
            target: target_path.as_str(),
            sources: entries.iter().map(|entry| entry.path.as_str()).collect(),
        };
        crate::app::command_handlers::print_json_report(&report, "config build output")?;
        return Ok(CommandOutput::new(()));
    }

    let panel = config_build_panel(source_path, &target_path, &entries);
    emit_human(false, &panel, HumanOutputMode::Compact, || Ok(()))?;

    Ok(CommandOutput::new(()))
}

fn config_build_panel(
    source_path: &Utf8Path,
    target_path: &Utf8Path,
    entries: &[ctx_traits_io::config_source::SourceEntry],
) -> Panel {
    let mut panel = Panel::new(
        "ctx",
        "config build",
        PanelStatus::Passed("passed".to_string()),
    )
    .row(PanelRow::toned(
        "source",
        source_path.as_str(),
        RowTone::Default,
    ))
    .row(PanelRow::toned(
        "target",
        target_path.as_str(),
        RowTone::Default,
    ));
    if !entries.is_empty() {
        let rows = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                PanelRow::toned(
                    format!("source {}", index + 1),
                    entry.path.as_str(),
                    RowTone::Default,
                )
            })
            .collect();
        panel = panel.section(PanelSection::new("sources", rows));
    }
    panel
}

/// Filter node's reported `file:` module-graph URLs to local files only:
/// drop anything containing a `node_modules` path component (the installed
/// `@ctx-traits/config` package deliberately never enters the manifest —
/// see the phase risk log). Every other local file node actually loaded —
/// regardless of how far outside `config_dir` it lives — is re-expressed
/// relative to `config_dir` (the config file's own directory; `../`
/// entries are exactly how the manifest format already expresses this, so
/// the same rule works identically for the repo-local and global layers)
/// and manifested. There is deliberately no "outside the repo" filter here:
/// an earlier version silently dropped any import the manifest format
/// could have expressed just fine (a shared module imported from above
/// `.ctx/`, or above a global layer's config-home directory, which has no
/// repository at all) — which built a `config.toml` whose manifest didn't
/// cover its own module graph, so the drift guard could never catch an
/// edit to the dropped file. A node-reported local file that cannot be
/// canonicalized (already gone by the time this runs) is a hard build
/// error naming the file, never a silent skip — the same "never silently
/// incomplete" invariant.
fn local_source_entries(
    sources: &[String],
    config_dir: &Utf8Path,
) -> crate::Result<Vec<ctx_traits_io::config_source::SourceEntry>> {
    let mut seen: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    let mut entries = Vec::new();
    for source in sources {
        let Some(raw_path) = source.strip_prefix("file://") else {
            continue;
        };
        let absolute = Utf8PathBuf::from(percent_decode(raw_path));
        if absolute
            .components()
            .any(|component| component.as_str() == "node_modules")
        {
            continue;
        }
        let canonical = absolute
            .canonicalize_utf8()
            .map_err(|error| crate::Error::Command {
                message: format!(
                    "config module loaded {absolute}, which cannot be manifested: {error}"
                ),
            })?;
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let relative = relative_path(config_dir, &canonical)?;
        let digest = ctx_traits_io::config_source::hash_file(&canonical)?;
        entries.push(ctx_traits_io::config_source::SourceEntry {
            path: relative,
            digest,
        });
    }
    Ok(entries)
}

/// Minimal percent-decoding for the small set of characters likely to
/// appear in a repository-local module path (space and a handful of
/// punctuation); anything else passes through as-is.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3])
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Express `target` relative to `base`, forward-slash separated. Both must
/// already be canonical absolute paths.
fn relative_path(base: &Utf8Path, target: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let base_components: Vec<&str> = base.components().map(|c| c.as_str()).collect();
    let target_components: Vec<&str> = target.components().map(|c| c.as_str()).collect();
    let mut common = 0;
    while common < base_components.len()
        && common < target_components.len()
        && base_components[common] == target_components[common]
    {
        common += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..base_components.len() {
        parts.push("..".to_string());
    }
    for component in &target_components[common..] {
        parts.push((*component).to_string());
    }
    if parts.is_empty() {
        return Err(crate::Error::Command {
            message: format!("{target} resolves to the same path as {base}"),
        });
    }
    Ok(Utf8PathBuf::from(parts.join("/")))
}

/// Convert the validated config JSON into sorted-key TOML text: strip
/// `null`s (JSON `null` has no TOML form and every `Option` field defaults),
/// sort every table's keys, and render through the `toml` crate. Not
/// `RuntimeConfig::serialize` — only part of the tree derives `Serialize`
/// today, and JSON-value emission gives byte-idempotent rebuilds for free,
/// independent of JS object insertion order.
fn render_sorted_toml(config: &serde_json::Value) -> crate::Result<String> {
    let value = json_to_toml(config).ok_or_else(|| crate::Error::Command {
        message: "config build produced an empty document".to_string(),
    })?;
    toml::to_string_pretty(&value).map_err(|source| crate::Error::Command {
        message: format!("failed to render generated config.toml: {source}"),
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConfigInitReportJson<'a> {
    package_json: &'a str,
}

/// `ctx traits config init --global` (0178): scaffold the config-home
/// `traits/` directory with a minimal `package.json` pinning
/// `@ctx-traits/config`, so a global `traits/runtime.ts` can resolve the
/// authoring package once the operator runs their package manager's
/// install — that install itself stays the operator's own step, never run
/// here.
pub(crate) fn handle_config_init(global: bool, json: bool) -> crate::Result<CommandOutput<()>> {
    if !global {
        return Err(crate::Error::Command {
            message: "config init requires --global — the repo tier has no scaffold command (0179 owns init/layout rework)".to_string(),
        });
    }

    let ctx_dir = ctx_traits_io::state::global_ctx_root()?;
    let traits_dir = ctx_dir.join("traits");
    let package_json_path = traits_dir.join("package.json");

    if !package_json_path.exists() {
        // Pinned to `packages/config/package.json`'s current version; the
        // operator resolves it through their package manager's usual
        // registry/link configuration — this scaffold only makes the
        // dependency declaration resolvable, it never runs the install.
        const CONFIG_PACKAGE_VERSION: &str = "0.1.0-alpha.0";
        let contents = format!(
            "{{\n  \"private\": true,\n  \"dependencies\": {{\n    \"@ctx-traits/config\": \"{CONFIG_PACKAGE_VERSION}\"\n  }}\n}}\n",
        );
        ctx_traits_io::write::write_build_output(&package_json_path, &contents)?;
    }

    if json {
        let report = ConfigInitReportJson {
            package_json: package_json_path.as_str(),
        };
        crate::app::command_handlers::print_json_report(&report, "config init output")?;
        return Ok(CommandOutput::new(()));
    }

    let panel = Panel::new(
        "ctx",
        "config init --global",
        PanelStatus::Passed("scaffolded".to_string()),
    )
    .row(PanelRow::toned(
        "package.json",
        package_json_path.as_str(),
        RowTone::Default,
    ));
    emit_human(false, &panel, HumanOutputMode::Compact, || {
        println!(
            "run your package manager's install in {traits_dir} to resolve @ctx-traits/config before authoring traits/runtime.ts"
        );
        Ok(())
    })?;

    Ok(CommandOutput::new(()))
}

fn json_to_toml(value: &serde_json::Value) -> Option<toml::Value> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(value) => Some(toml::Value::Boolean(*value)),
        serde_json::Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Some(toml::Value::Integer(value))
            } else {
                number.as_f64().map(toml::Value::Float)
            }
        }
        serde_json::Value::String(value) => Some(toml::Value::String(value.clone())),
        serde_json::Value::Array(items) => Some(toml::Value::Array(
            items.iter().filter_map(json_to_toml).collect(),
        )),
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut table = toml::map::Map::new();
            for key in keys {
                if let Some(value) = json_to_toml(&map[key]) {
                    table.insert(key.clone(), value);
                }
            }
            Some(toml::Value::Table(table))
        }
    }
}
