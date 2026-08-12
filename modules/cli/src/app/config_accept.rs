//! CLI orchestration for `ctx traits config accept` (0178): the operator
//! step that accepts a repo-committed `runtime.example.ts`/`.toml`, shows
//! its content, materializes the machine-local `runtime.ts`/`.toml` copy,
//! and records a digest stamp in the trust store. Never auto-accepts
//! non-interactively without `--yes` — acceptance is the operator's own
//! decision, never a running loop's.

use std::io::IsTerminal;

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::response::CommandOutput;

use crate::app::presentation::{
    HumanOutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConfigAcceptReportJson<'a> {
    example: &'a str,
    machine: &'a str,
    digest: &'a str,
}

/// Resolve the example/machine-copy path pair for the repo tier rooted at
/// `repo_root`.
pub(crate) fn repo_paths(repo_root: &Utf8Path) -> (Utf8PathBuf, Utf8PathBuf, Utf8PathBuf) {
    (
        repo_root.join(
            ctx_traits_io::layout::RUNTIME_CONFIG_EXAMPLE_TS
                .strip_prefix("./")
                .unwrap_or(ctx_traits_io::layout::RUNTIME_CONFIG_EXAMPLE_TS),
        ),
        repo_root.join(
            ctx_traits_io::layout::RUNTIME_CONFIG_EXAMPLE
                .strip_prefix("./")
                .unwrap_or(ctx_traits_io::layout::RUNTIME_CONFIG_EXAMPLE),
        ),
        repo_root.join(
            ctx_traits_io::layout::RUNTIME_CONFIG_SOURCE
                .strip_prefix("./")
                .unwrap_or(ctx_traits_io::layout::RUNTIME_CONFIG_SOURCE),
        ),
    )
}

/// Machine copy path matching the example's own extension (`.ts` examples
/// materialize to [`ctx_traits_io::layout::RUNTIME_CONFIG_SOURCE`], `.toml`
/// examples to [`ctx_traits_io::layout::RUNTIME_CONFIG`]).
fn machine_path_for(repo_root: &Utf8Path, example_path: &Utf8Path) -> Utf8PathBuf {
    if example_path.extension() == Some("ts") {
        repo_root.join(
            ctx_traits_io::layout::RUNTIME_CONFIG_SOURCE
                .strip_prefix("./")
                .unwrap_or(ctx_traits_io::layout::RUNTIME_CONFIG_SOURCE),
        )
    } else {
        repo_root.join(
            ctx_traits_io::layout::RUNTIME_CONFIG
                .strip_prefix("./")
                .unwrap_or(ctx_traits_io::layout::RUNTIME_CONFIG),
        )
    }
}

pub(crate) fn handle_config_accept(yes: bool, json: bool) -> crate::Result<CommandOutput<()>> {
    let cwd = Utf8PathBuf::from_path_buf(std::env::current_dir().map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: ".".to_string(),
            source,
        })
    })?)
    .map_err(|_| crate::Error::Command {
        message: "current directory is not valid UTF-8".to_string(),
    })?;
    // `stable_repo_root` treats its argument as a file path and probes from
    // its *parent* — join a synthetic leaf so the probe starts at `cwd`
    // itself, not `cwd`'s parent.
    let repo_root = crate::app::cdk_build::stable_repo_root(&cwd.join(".ctx-accept-probe"))?;
    let (example_ts, example_toml, _source) = repo_paths(&repo_root);

    let Some(example_path) =
        ctx_traits_io::runtime_acceptance::find_example(&example_ts, &example_toml)?
    else {
        return Err(crate::Error::Command {
            message: format!(
                "no {example_ts} or {example_toml} is committed in this repo — nothing to accept"
            ),
        });
    };

    let repo_key =
        ctx_traits_io::state::repo_key(&ctx_traits_io::state::canonical_repo_root(&repo_root)?);
    let acceptance = ctx_traits_io::runtime_acceptance::check_acceptance(&example_path, &repo_key)?;
    if matches!(
        acceptance,
        ctx_traits_io::runtime_acceptance::Acceptance::Accepted
    ) {
        return Err(crate::Error::Command {
            message: format!("{example_path} is already accepted — nothing to do"),
        });
    }

    let content = ctx_traits_io::read::read_text(&example_path)?;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if !interactive && !yes {
        return Err(crate::Error::Command {
            message: format!(
                "accepting {example_path} requires an interactive terminal or `--yes` — acceptance is an operator decision, never automatic"
            ),
        });
    }

    let is_reacceptance = matches!(
        acceptance,
        ctx_traits_io::runtime_acceptance::Acceptance::NeedsAcceptance {
            prior_digest: Some(_),
            ..
        }
    );
    let preview = if is_reacceptance {
        match ctx_traits_io::runtime_acceptance::stashed_bytes(&repo_key)? {
            Some(prior_bytes) => match String::from_utf8(prior_bytes) {
                Ok(prior_text) if prior_text != content => render_diff(&prior_text, &content),
                _ => content.clone(),
            },
            None => content.clone(),
        }
    } else {
        content.clone()
    };

    let machine_path = machine_path_for(&repo_root, &example_path);
    let digest =
        ctx_traits_io::runtime_acceptance::accept(&example_path, &machine_path, &repo_key)?;

    if machine_path.extension() == Some("ts") {
        crate::app::config_build::handle_config_build(Some(machine_path.as_str()), true)?;
    }

    if json {
        let report = ConfigAcceptReportJson {
            example: example_path.as_str(),
            machine: machine_path.as_str(),
            digest: &digest,
        };
        crate::app::command_handlers::print_json_report(&report, "config accept output")?;
        return Ok(CommandOutput::new(()));
    }

    let panel = Panel::new(
        "ctx",
        "config accept",
        PanelStatus::Passed("accepted".to_string()),
    )
    .row(PanelRow::toned(
        "example",
        example_path.as_str(),
        RowTone::Default,
    ))
    .row(PanelRow::toned(
        "machine",
        machine_path.as_str(),
        RowTone::Default,
    ))
    .row(PanelRow::toned("digest", &digest, RowTone::Default));
    let preview_ref = &preview;
    emit_human(false, &panel, HumanOutputMode::Compact, || {
        println!("{preview_ref}");
        Ok(())
    })?;

    Ok(CommandOutput::new(()))
}

/// Minimal line-oriented diff for the re-acceptance preview: unchanged
/// lines are shown, added/removed lines are prefixed `+`/`-`. Not a full
/// LCS diff — good enough to show the operator what changed without
/// pulling in a diff crate for one CLI preview.
fn render_diff(prior: &str, current: &str) -> String {
    let prior_lines: Vec<&str> = prior.lines().collect();
    let current_lines: Vec<&str> = current.lines().collect();
    let mut out = String::new();
    let max = prior_lines.len().max(current_lines.len());
    for i in 0..max {
        match (prior_lines.get(i), current_lines.get(i)) {
            (Some(p), Some(c)) if p == c => {
                out.push_str("  ");
                out.push_str(p);
                out.push('\n');
            }
            (Some(p), Some(c)) => {
                out.push_str("- ");
                out.push_str(p);
                out.push('\n');
                out.push_str("+ ");
                out.push_str(c);
                out.push('\n');
            }
            (Some(p), None) => {
                out.push_str("- ");
                out.push_str(p);
                out.push('\n');
            }
            (None, Some(c)) => {
                out.push_str("+ ");
                out.push_str(c);
                out.push('\n');
            }
            (None, None) => {}
        }
    }
    out
}
