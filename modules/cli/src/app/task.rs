//! `ctx traits task import` handler: markdown board file → canonical TOML
//! task document.

use camino::Utf8PathBuf;
use ctx_traits_core::response::{CommandOutput, Envelope};

use crate::app::command_handlers::print_json_report;
use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct TaskImportReport {
    source: String,
    written: String,
    key: String,
    title: String,
    status: Option<String>,
}

pub(crate) fn handle_task_import(path: &str, json: bool) -> crate::Result<CommandOutput<()>> {
    let source_path = Utf8PathBuf::from(path);
    let target_path = source_path.with_extension("toml");
    if target_path.exists() {
        return Err(crate::Error::Command {
            message: format!("{target_path} already exists — refusing to overwrite"),
        });
    }

    let source_text = ctx_traits_io::read::read_text(&source_path)?;
    let document = ctx_traits_core::task::import_markdown(&source_text)?;
    let output_text = ctx_traits_core::task::serialize(&document)?;
    ctx_traits_io::write::write_text(&target_path, &output_text)?;

    let report = TaskImportReport {
        source: source_path.to_string(),
        written: target_path.to_string(),
        key: document.key.clone(),
        title: document.title.clone(),
        status: document
            .status
            .map(|status| format!("{status:?}").to_lowercase()),
    };

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&report), "task import report")?;
        }
        OutputMode::Human(mode) => {
            let panel = Panel::new(
                "ctx",
                "task import",
                PanelStatus::Passed("passed".to_string()),
            )
            .row(PanelRow::toned("source", &report.source, RowTone::Default))
            .row(PanelRow::toned(
                "written",
                &report.written,
                RowTone::Default,
            ))
            .row(PanelRow::toned("key", &report.key, RowTone::Default))
            .row(PanelRow::toned("title", &report.title, RowTone::Default))
            .row(PanelRow::toned(
                "status",
                report.status.as_deref().unwrap_or("(none)"),
                RowTone::Default,
            ));
            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}
