//! `ctx traits config`: presentation of the center's served config answer.

use camino::Utf8PathBuf;
use ctx_traits_core::response::CommandOutput;
use ctx_traits_io::config_view::{ConfigResolution, ConfigView, ConfigWinnerWire};

use crate::app::command_handlers::print_json_report;
use crate::app::tui::write_plain_line;

pub(crate) fn handle_config_report(json: bool) -> crate::Result<CommandOutput<()>> {
    let cwd = std::env::current_dir().map_err(|error| crate::Error::Command {
        message: format!("read current directory: {error}"),
    })?;
    let scope = Utf8PathBuf::from_path_buf(cwd).map_err(|path| crate::Error::Command {
        message: format!("current directory is not valid UTF-8: {}", path.display()),
    })?;
    let answer = ctx_traits_io::center::run_config_request(scope.as_str())?;
    let view = match &answer.resolution {
        ConfigResolution::Resolved(view) => view,
        ConfigResolution::Refused { reason } | ConfigResolution::Failed { reason } => {
            return Err(crate::Error::Command {
                message: reason.clone(),
            });
        }
    };

    if json {
        print_json_report(&answer, "config report")?;
    } else {
        emit_plain(view)?;
    }
    Ok(CommandOutput::new(()))
}

fn emit_winner(label: &str, winner: &ConfigWinnerWire) -> crate::Result<()> {
    let origin = winner.source.as_deref().unwrap_or("built-in defaults");
    write_plain_line(format!(
        "    {label} provenance: {} [{}; {}]",
        origin, winner.layer, winner.reason
    ))
}

fn emit_plain(view: &ConfigView) -> crate::Result<()> {
    write_plain_line("ctx traits config")?;
    write_plain_line("  seats:")?;
    if view.seats.is_empty() {
        write_plain_line("    (none configured)")?;
    }
    for seat in &view.seats {
        let name = seat
            .seat_index
            .map(|index| format!("{}.{}", seat.role, index))
            .unwrap_or_else(|| seat.role.clone());
        write_plain_line(format!("    {name}:"))?;
        match &seat.model {
            Some(model) => {
                write_plain_line(format!("      model: {model}"))?;
                if let Some(winner) = &seat.model_winner {
                    emit_winner("model", winner)?;
                }
            }
            None => write_plain_line("      model: not configured")?,
        }
        match &seat.reasoning_effort {
            Some(effort) => {
                write_plain_line(format!("      reasoning effort: {effort}"))?;
                if let Some(winner) = &seat.effort_winner {
                    emit_winner("reasoning effort", winner)?;
                }
            }
            None => write_plain_line("      reasoning effort: not configured")?,
        }
    }

    write_plain_line("  runtime:")?;
    for row in &view.runtime {
        write_plain_line(format!(
            "    {}: {} [{}]",
            row.name, row.value, row.qualifier
        ))?;
    }
    write_plain_line(format!(
        "  approved traits: {} ({} distinct digests)",
        view.trust.approved_members.join(", "),
        view.trust.approved_digests
    ))?;
    write_plain_line(format!("  documents: {}", view.documents.len()))?;
    if view.documents.is_empty() {
        write_plain_line("    built-in defaults (no runtime document)")?;
    }
    for document in &view.documents {
        write_plain_line(format!("    {}: {}", document.layer, document.path))?;
    }
    for warning in &view.tier_warnings {
        write_plain_line(format!("  warning: {warning}"))?;
    }
    write_plain_line("  verdict: resolved")?;
    write_plain_line(format!(
        "  instant epoch millis: {}",
        view.instant_epoch_millis
    ))
}
