//! `ConfigDocument` (0177): the committed, repo-only, declarative project
//! config — `.ctx/traits/config.toml`, either hand-authored or compiled from
//! `.ctx/traits/config.ts` into `.ctx/traits/generated/config.toml`.
//!
//! Today's content is exactly [`ProjectManifest`] under `[vendor]` (formerly
//! the standalone `vendor.toml`). Team-level trait setting overrides and
//! dispatch defaults are named in the task's scope but not yet folded into
//! this schema — see task 0177's work summary for that open item. The
//! schema is `deny_unknown_fields` and, deliberately, cannot express argv,
//! commands, bins, or gates: executable facts are 0178's document.
//!
//! `[vendor]` is repo-only: resolving a config document at a global-scope
//! path with a `[vendor]` table is a refusal (dependencies are project
//! identity, never a machine's).

use camino::Utf8Path;

use ctx_traits_core::manifest::ProjectManifest;

use crate::layout;

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ConfigDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<ProjectManifest>,
}

fn refusal(message: impl Into<String>) -> crate::Error {
    crate::Error::Core(
        ctx_traits_core::manifest::Error::InvalidField {
            field_path: "config".to_string(),
            message: message.into(),
        }
        .into(),
    )
}

/// Resolve the config document path under `repo_root`, honoring the
/// `config.ts` -> `generated/config.toml` pathway over the hand-authored
/// `.ctx/traits/config.toml`, and refusing when both a source and a hand
/// file are present (one authority per fact) or when the source exists with
/// no built artifact yet.
pub fn resolve_document_path(repo_root: &Utf8Path) -> crate::Result<Option<camino::Utf8PathBuf>> {
    let source_path = repo_root.join(layout::CONFIG_SOURCE);
    let hand_path = repo_root.join(layout::PROJECT_CONFIG);
    let generated_path = layout::trait_generated_root_path(repo_root).join("config.toml");

    if source_path.exists() {
        if hand_path.exists() {
            return Err(refusal(format!(
                "both {source_path} and hand-authored {hand_path} are present — one authority per fact: delete one"
            )));
        }
        crate::config_source::guard_never_built(&source_path, &generated_path)?;
        if !generated_path.exists() {
            return Ok(None);
        }
        let text = crate::read::read_text(&generated_path)?;
        let source_dir = source_path
            .parent()
            .unwrap_or_else(|| Utf8Path::new(""))
            .to_path_buf();
        crate::config_source::guard_generated_config(&generated_path, &text, &source_dir)?;
        return Ok(Some(generated_path));
    }

    if hand_path.exists() {
        return Ok(Some(hand_path));
    }

    Ok(None)
}

/// Decode `text` (read from `path`, used only for error context) as a
/// [`ConfigDocument`]. The single decode site for the schema — every reader
/// that holds a resolved path routes through this instead of re-deriving
/// the `toml::from_str` + error-context block.
pub fn decode(path: &Utf8Path, text: &str) -> crate::Result<ConfigDocument> {
    toml::from_str(text).map_err(|source| {
        crate::Error::from(crate::parse::Error::TomlDecode {
            context: path.to_string(),
            source,
        })
    })
}

/// Read and decode the config document under `repo_root`. The `[vendor]`-is-
/// repo-only refusal for global-scope config lives at the build-time write
/// site (`ctx traits config build`, `config_build.rs`) — the only place a
/// global-scope document is ever produced — not here.
pub fn read_document(repo_root: &Utf8Path) -> crate::Result<Option<ConfigDocument>> {
    let Some(path) = resolve_document_path(repo_root)? else {
        return Ok(None);
    };
    let text = crate::read::read_text(&path)?;
    let document = decode(&path, &text)?;
    Ok(Some(document))
}
