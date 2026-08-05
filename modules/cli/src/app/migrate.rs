//! `ctx traits migrate` handler: mechanical version-to-version migration.
//!
//! Mirrors the `doctor migrate-state`/`doctor migrate-config` plan/apply
//! shape (`crate::app::doctor`): plan mode reports a diff and never writes;
//! `--apply` writes only after gates (the migration engine's own refusal
//! checks, run inside `ctx_traits_core::migrate::plan_migration`) pass.

use ctx_traits_core::response::{CommandOutput, Envelope};
use serde::Serialize;

use crate::app::command_handlers::print_json_report;
use crate::app::presentation::{
    OutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct MigrateReport {
    action: &'static str,
    path: String,
    from: String,
    to: String,
    changed: bool,
    diff: Vec<String>,
    source_digest_before: String,
    source_digest_after: String,
    canonical_digest_before: String,
    canonical_digest_after: String,
    assisted_needed: Vec<AssistedItemReport>,
    written: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct AssistedItemReport {
    step: &'static str,
    excerpt: String,
    reason: String,
}

pub(crate) fn handle_migrate(
    id_or_path: &str,
    to: Option<&str>,
    apply: bool,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let path = if camino::Utf8Path::new(id_or_path).extension().is_none() {
        ctx_traits_io::run::resolve_trait_path(None, Some(id_or_path), "migrate")?.0
    } else {
        camino::Utf8PathBuf::from(id_or_path)
    };
    let source_text = ctx_traits_io::read::read_text(&path)?;

    let target = to.map(str::to_string).unwrap_or_else(|| {
        ctx_traits_core::r#trait::SUPPORTED_SCHEMA_VERSIONS
            .last()
            .copied()
            .expect("SUPPORTED_SCHEMA_VERSIONS is never empty")
            .to_string()
    });

    let plan = ctx_traits_core::migrate::plan_migration(&source_text, &target)
        .map_err(ctx_traits_core::Error::from)?;

    if apply && !plan.assisted_needed.is_empty() {
        return Err(crate::Error::Command {
            message: format!(
                "{path}: migration to {target} requires assisted migration for {} construct(s) (not yet implemented) — refusing to apply a partial mechanical rewrite",
                plan.assisted_needed.len()
            ),
        });
    }

    let written = if apply && !plan.is_noop() {
        ctx_traits_io::write::write_text(&path, &plan.output_text)?;
        true
    } else {
        false
    };

    let report = MigrateReport {
        action: if apply { "apply" } else { "plan" },
        path: path.to_string(),
        from: plan.from.clone(),
        to: plan.to.clone(),
        changed: !plan.is_noop(),
        diff: diff_lines(&plan.source_text, &plan.output_text),
        source_digest_before: plan.source_digest_before.to_string(),
        source_digest_after: plan.source_digest_after.to_string(),
        canonical_digest_before: plan.canonical_digest_before.to_string(),
        canonical_digest_after: plan.canonical_digest_after.to_string(),
        assisted_needed: plan
            .assisted_needed
            .iter()
            .map(|item| AssistedItemReport {
                step: item.step,
                excerpt: item.excerpt.clone(),
                reason: item.reason.clone(),
            })
            .collect(),
        written,
    };

    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&Envelope::ok(&report), "migrate report")?;
        }
        OutputMode::Human(mode) => {
            let outcome_text = if apply {
                if written {
                    "written"
                } else {
                    "no changes — nothing written"
                }
            } else {
                "plan only — nothing written; re-run with --apply to write"
            };

            let mut panel = Panel::new("ctx", "migrate", PanelStatus::Passed("passed".to_string()))
                .row(PanelRow::toned("path", &report.path, RowTone::Default))
                .row(PanelRow::toned("from", &report.from, RowTone::Default))
                .row(PanelRow::toned("to", &report.to, RowTone::Default))
                .row(PanelRow::toned(
                    "source-digest",
                    format!(
                        "{} -> {}",
                        report.source_digest_before, report.source_digest_after
                    ),
                    RowTone::Default,
                ))
                .row(PanelRow::toned(
                    "canonical-digest",
                    format!(
                        "{} -> {}",
                        report.canonical_digest_before, report.canonical_digest_after
                    ),
                    RowTone::Default,
                ));

            if !report.diff.is_empty() {
                let rows = report
                    .diff
                    .iter()
                    .map(|line| {
                        if let Some(removed) = line.strip_prefix("- ") {
                            PanelRow::toned("removed", removed, RowTone::Fail)
                        } else if let Some(added) = line.strip_prefix("+ ") {
                            PanelRow::toned("added", added, RowTone::Pass)
                        } else {
                            PanelRow::toned("changed", line, RowTone::Default)
                        }
                    })
                    .collect();
                panel = panel.section(PanelSection::new("diff", rows));
            }

            if report.canonical_digest_before != report.canonical_digest_after {
                panel = panel.row(PanelRow::toned(
                    "trust",
                    format!(
                        "canonical digest moved; re-approval required — run `ctx traits trust approve {}`",
                        report.path
                    ),
                    RowTone::Warn,
                ));
            }

            if !report.assisted_needed.is_empty() {
                let rows = report
                    .assisted_needed
                    .iter()
                    .map(|item| {
                        PanelRow::toned(
                            item.step,
                            format!("{}: {}", item.reason, item.excerpt),
                            RowTone::Warn,
                        )
                    })
                    .collect();
                panel = panel.section(PanelSection::new("assisted-needed", rows));
            }

            panel = panel.row(PanelRow::toned("outcome", outcome_text, RowTone::Default));

            emit_human(false, &panel, mode, || Ok(()))?;
        }
    }

    Ok(CommandOutput::new(()))
}

/// Minimal reviewable line diff: an LCS-based line alignment so inserted and
/// deleted lines (not just same-index replacements) render correctly, with
/// each line prefixed `-`/`+`. Unchanged lines are omitted — the mechanical
/// engine's contract is that untouched spans never appear in the diff.
fn diff_lines(before: &str, after: &str) -> Vec<String> {
    let before: Vec<&str> = before.lines().collect();
    let after: Vec<&str> = after.lines().collect();
    let n = before.len();
    let m = after.len();

    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if before[i] == after[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if before[i] == after[j] {
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(format!("- {}", before[i]));
            i += 1;
        } else {
            out.push(format!("+ {}", after[j]));
            j += 1;
        }
    }
    while i < n {
        out.push(format!("- {}", before[i]));
        i += 1;
    }
    while j < m {
        out.push(format!("+ {}", after[j]));
        j += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::diff_lines;

    #[test]
    fn diff_lines_reports_only_changed_lines() {
        let before = "a\nb\nc\n";
        let after = "a\nx\nc\n";
        assert_eq!(
            diff_lines(before, after),
            vec!["- b".to_string(), "+ x".to_string()]
        );
    }

    #[test]
    fn diff_lines_handles_insert_and_delete() {
        let before = "a\nb\nc\n";
        let after = "a\nc\nd\n";
        assert_eq!(
            diff_lines(before, after),
            vec!["- b".to_string(), "+ d".to_string()]
        );
    }

    #[test]
    fn diff_lines_empty_for_identical_input() {
        assert!(diff_lines("a\nb\n", "a\nb\n").is_empty());
    }
}
