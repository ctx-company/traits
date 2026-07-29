//! `ctx traits doctor`: a read-only aggregate report over a folder of
//! Agent-Skills-style source files (`SKILL.md`/`AGENTS.md`/`CLAUDE.md`).
//!
//! Doctor performs no analysis of its own. Every fact in a [`DoctorReport`]
//! is aggregated from data the existing import planner
//! ([`super::plan_agent_skills_import`]) and the shared scaffold builder
//! already produce for each file, plus purely structural cross-file
//! comparisons (canonical trait-ID collisions, byte-equal normalized-summary
//! collisions) computed over that data. Doctor never fabricates a pass/fail
//! verdict, a freshness verdict, or new keyword/semantic analysis: `doctor`
//! suggests, `check` verifies.
//!
//! This module is pure: no filesystem access, no wall-clock, no model calls.
//! IO discovery and file reads live in `ctx-traits-io`; the CLI command
//! orchestrates both and renders the result.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::{Finding, Severity};
use crate::scaffold::{ScaffoldDeclaration, ScaffoldDependencyEdge, TraitScaffold};

use super::{ImportReport, ImportWarning, InferredField, ReviewAction, UnsupportedField};

/// Schema version tag for [`DoctorReport`], carried in the JSON output so
/// downstream tooling can detect shape changes across releases.
pub const DOCTOR_REPORT_SCHEMA_VERSION: &str = "doctor/v1";

/// One discovered file's outcome, as supplied by the caller (`ctx-traits-cli`)
/// after running the deterministic import planner against its content.
/// Doctor performs no IO of its own; the caller has already read the file and
/// attempted to plan it before calling [`build_doctor_report`].
#[derive(Debug, Clone)]
pub struct DoctorFileInput {
    /// Path relative to the doctor root, forward-slash separated.
    pub path: String,
    pub outcome: DoctorFileOutcome,
}

/// The result of attempting to read and plan one discovered file.
#[derive(Debug, Clone)]
pub enum DoctorFileOutcome {
    /// The file could not be read (unreadable, non-UTF-8, symlink, etc.).
    ReadError { message: String },
    /// The file was read but the deterministic import planner rejected it.
    PlanError { message: String },
    /// The file was read and planned successfully.
    Analyzed {
        trait_id: String,
        trait_name: String,
        summary: String,
        report: Box<ImportReport>,
        /// The same source-anchored `TraitScaffold` `import` would build for
        /// this candidate (same construction, same evidence).
        scaffold: Box<TraitScaffold>,
        /// True when this candidate was analyzed as a directory-shaped
        /// multi-file source (its containing directory has discoverable
        /// linked resources), matching what `ctx traits import --source
        /// <dir>` would do. Used only to build the copy-pasteable next
        /// action's `--source` argument.
        source_is_directory: bool,
    },
}

/// Aggregate doctor report for a folder of Agent-Skills-style source files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DoctorReport {
    pub schema_version: String,
    /// Display path of the root that was inspected.
    pub root: String,
    pub entries: Vec<DoctorEntry>,
    pub summary: DoctorSummary,
    /// Cross-file structural collisions (trait-ID, normalized summary).
    pub collisions: Vec<DoctorCollision>,
    /// One structured next action per successfully analyzed candidate: a
    /// copy-pasteable `ctx traits import --source <path>` command for
    /// `SKILL.md`-named candidates (directory-form when linked resources
    /// were discovered), or an explanatory caveat (no command) for
    /// `AGENTS.md`/`CLAUDE.md` candidates, which import's file-name guard
    /// currently rejects outright.
    pub next_actions: Vec<DoctorNextAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DoctorFileStatus {
    Analyzed,
    ReadError,
    PlanError,
}

/// Per-file doctor findings and evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DoctorEntry {
    pub path: String,
    pub status: DoctorFileStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Raw source digest, verbatim from `ImportReport.raw_source_digest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_source_digest: Option<String>,
    /// Multi-file import evidence (entry file, included files, resource
    /// mappings, graph digest), verbatim from `ImportReport.multi_file_evidence`
    /// when this candidate was analyzed as a directory-shaped source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_file_evidence: Option<super::MultiFileEvidence>,
    /// Fields the import planner inferred from the raw source, verbatim from
    /// `ImportReport.inferred_fields`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inferred_fields: Vec<InferredField>,
    /// Fields the import planner could not canonicalize, verbatim from
    /// `ImportReport.unsupported_fields`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported_fields: Vec<UnsupportedField>,
    /// Recommended review actions, verbatim from `ImportReport.review_actions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_actions: Vec<ReviewAction>,
    /// Deterministic conversion warnings, verbatim from
    /// `ImportReport.conversion_warnings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversion_warnings: Vec<String>,
    /// Structured import warnings, verbatim from `ImportReport.warnings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import_warnings: Vec<ImportWarning>,
    /// Concrete hidden-content scanner findings, carrying their existing
    /// Critical/Warning/Advisory severities unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_content_findings: Vec<Finding>,
    /// Doctor-specific funnel explanations (missing verification evidence,
    /// untyped outputs, ID/summary collisions, staleness-unknown, etc).
    /// Always advisory: doctor suggests, it never fails a file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advisories: Vec<DoctorAdvisory>,
    /// The same source-anchored scaffold declarations `import` would build,
    /// each carrying its own confidence and source anchor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scaffold_declarations: Vec<ScaffoldDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scaffold_dependency_edges: Vec<ScaffoldDependencyEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scaffold_review_warnings: Vec<String>,
    /// Always true today: every scaffold requires a subsequent
    /// deterministic `ctx traits check` before it is trusted.
    pub scaffold_check_required: bool,
    /// True when this candidate was analyzed as a directory-shaped
    /// multi-file source (its containing directory has discoverable linked
    /// resources), matching what `ctx traits import --source <dir>` would
    /// do. Drives whether the next action's `--source` argument is the file
    /// or its containing directory.
    #[serde(default)]
    pub source_is_directory: bool,
    pub freshness: DoctorFreshness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DoctorAdvisory {
    pub code: String,
    pub message: String,
}

/// Doctor never touches the clock or filesystem timestamps, so freshness is
/// always reported as an explicit unknown rather than a fabricated verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DoctorFreshness {
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DoctorSummary {
    pub files_total: usize,
    pub files_analyzed: usize,
    pub files_errored: usize,
    pub critical_findings: usize,
    pub warning_findings: usize,
    pub advisory_findings: usize,
    /// Sum of `scaffold_declarations.len()` across every analyzed candidate;
    /// what "what import would give you" summaries quote as the total typed
    /// declaration count an import run across this root would produce.
    pub total_scaffold_declarations: usize,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DoctorCollisionKind {
    TraitIdCollision,
    NormalizedSummaryCollision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DoctorCollision {
    pub kind: DoctorCollisionKind,
    pub key: String,
    pub paths: Vec<String>,
}

/// One structured, copy-pasteable next action for a successfully analyzed
/// doctor candidate. Exactly one of `command`/`caveat` carries the action:
/// `SKILL.md`-named candidates get a runnable, shell-quoted `command` and no
/// caveat; `AGENTS.md`/`CLAUDE.md` candidates get an explanatory `caveat`
/// and no command, because import's file-name guard rejects them outright
/// today and a command that always fails is not a next action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DoctorNextAction {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caveat: Option<String>,
}

/// Build the aggregate [`DoctorReport`] from already-read, already-planned
/// per-file inputs. Pure and deterministic: identical inputs always produce
/// byte-identical output (modulo the caller's own JSON serialization).
pub fn build_doctor_report(root: &str, inputs: Vec<DoctorFileInput>) -> DoctorReport {
    let mut entries: Vec<DoctorEntry> = inputs.into_iter().map(doctor_entry).collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let collisions = collect_collisions(&entries);
    apply_collision_advisories(&mut entries, &collisions);

    let mut summary = DoctorSummary {
        files_total: entries.len(),
        files_analyzed: 0,
        files_errored: 0,
        critical_findings: 0,
        warning_findings: 0,
        advisory_findings: 0,
        total_scaffold_declarations: 0,
    };
    for entry in &entries {
        match entry.status {
            DoctorFileStatus::Analyzed => {
                summary.files_analyzed += 1;
                summary.total_scaffold_declarations += entry.scaffold_declarations.len();
            }
            DoctorFileStatus::ReadError | DoctorFileStatus::PlanError => {
                summary.files_errored += 1;
            }
        }
        summary.advisory_findings += entry.advisories.len();
        for finding in &entry.hidden_content_findings {
            match finding.severity {
                Severity::Critical => summary.critical_findings += 1,
                Severity::Warning => summary.warning_findings += 1,
                Severity::Advisory => summary.advisory_findings += 1,
            }
        }
    }

    let next_actions = entries
        .iter()
        .filter(|entry| entry.status == DoctorFileStatus::Analyzed)
        .map(|entry| next_action_for(root, entry))
        .collect();

    DoctorReport {
        schema_version: DOCTOR_REPORT_SCHEMA_VERSION.to_string(),
        root: root.to_string(),
        entries,
        summary,
        collisions,
        next_actions,
    }
}

/// Structurally-derived (not keyword/semantic) advisory: a scaffold with no
/// `procedure-step` declarations has no typed, source-anchored verification
/// steps — only `check` can later confirm what those steps assert, but
/// there is nothing for it to walk today.
fn has_verification_steps(declarations: &[ScaffoldDeclaration]) -> bool {
    declarations
        .iter()
        .any(|declaration| declaration.kind == "procedure-step")
}

/// Structurally-derived advisory: a scaffold with no `port` declarations has
/// no typed boundary contract, so whatever this trait produces is an
/// untyped output as far as the scaffold can tell.
fn has_typed_outputs(declarations: &[ScaffoldDeclaration]) -> bool {
    declarations
        .iter()
        .any(|declaration| declaration.kind == "port")
}

fn doctor_entry(input: DoctorFileInput) -> DoctorEntry {
    let path = input.path;
    match input.outcome {
        DoctorFileOutcome::ReadError { message } => {
            empty_error_entry(path, DoctorFileStatus::ReadError, message)
        }
        DoctorFileOutcome::PlanError { message } => {
            empty_error_entry(path, DoctorFileStatus::PlanError, message)
        }
        DoctorFileOutcome::Analyzed {
            trait_id,
            trait_name,
            summary,
            report,
            scaffold,
            source_is_directory,
        } => {
            let mut advisories = vec![
                DoctorAdvisory {
                    code: "doctor.no-check-evidence".to_string(),
                    message: "no typed `ctx traits check` evidence exists for this candidate \
                              yet; import it, then run `ctx traits check` before trusting it"
                        .to_string(),
                },
                DoctorAdvisory {
                    code: "doctor.freshness-unknown".to_string(),
                    message: "doctor performs no filesystem-timestamp or wall-clock comparison; \
                              freshness/staleness of this source is unknown"
                        .to_string(),
                },
            ];
            if !has_verification_steps(&scaffold.declarations) {
                advisories.push(DoctorAdvisory {
                    code: "doctor.no-verification-steps".to_string(),
                    message: "the reconstructed scaffold declares no typed procedure steps; \
                              this candidate has no verification evidence for `ctx traits check` \
                              to walk once imported"
                        .to_string(),
                });
            }
            if !has_typed_outputs(&scaffold.declarations) {
                advisories.push(DoctorAdvisory {
                    code: "doctor.untyped-outputs".to_string(),
                    message: "the reconstructed scaffold declares no typed ports; this \
                              candidate's outputs are untyped until ports are added"
                        .to_string(),
                });
            }

            DoctorEntry {
                path,
                status: DoctorFileStatus::Analyzed,
                raw_source_digest: Some(report.raw_source_digest.as_str().to_string()),
                multi_file_evidence: report.multi_file_evidence.clone(),
                inferred_fields: report.inferred_fields.clone(),
                unsupported_fields: report.unsupported_fields.clone(),
                review_actions: report.review_actions.clone(),
                conversion_warnings: report.conversion_warnings.clone(),
                import_warnings: report.warnings.clone(),
                hidden_content_findings: report.hidden_content_findings.clone(),
                advisories,
                scaffold_declarations: scaffold.declarations.clone(),
                scaffold_dependency_edges: scaffold.dependency_edges.clone(),
                scaffold_review_warnings: scaffold.review_warnings.clone(),
                scaffold_check_required: scaffold.check.required,
                source_is_directory,
                freshness: DoctorFreshness::Unknown,
                error: None,
                trait_id: Some(trait_id),
                trait_name: Some(trait_name),
                summary: Some(summary),
            }
        }
    }
}

fn empty_error_entry(path: String, status: DoctorFileStatus, message: String) -> DoctorEntry {
    DoctorEntry {
        path,
        status,
        trait_id: None,
        trait_name: None,
        summary: None,
        raw_source_digest: None,
        multi_file_evidence: None,
        inferred_fields: Vec::new(),
        unsupported_fields: Vec::new(),
        review_actions: Vec::new(),
        conversion_warnings: Vec::new(),
        import_warnings: Vec::new(),
        hidden_content_findings: Vec::new(),
        advisories: Vec::new(),
        scaffold_declarations: Vec::new(),
        scaffold_dependency_edges: Vec::new(),
        scaffold_review_warnings: Vec::new(),
        scaffold_check_required: false,
        source_is_directory: false,
        freshness: DoctorFreshness::Unknown,
        error: Some(message),
    }
}

fn collect_collisions(entries: &[DoctorEntry]) -> Vec<DoctorCollision> {
    let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut summaries: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in entries {
        if let Some(trait_id) = &entry.trait_id {
            ids.entry(trait_id.clone())
                .or_default()
                .push(entry.path.clone());
        }
        if let Some(summary) = &entry.summary {
            summaries
                .entry(summary.trim().to_string())
                .or_default()
                .push(entry.path.clone());
        }
    }

    let mut collisions = Vec::new();
    for (key, mut paths) in ids {
        if paths.len() > 1 {
            paths.sort();
            paths.dedup();
            collisions.push(DoctorCollision {
                kind: DoctorCollisionKind::TraitIdCollision,
                key,
                paths,
            });
        }
    }
    for (key, mut paths) in summaries {
        if paths.len() > 1 {
            paths.sort();
            paths.dedup();
            collisions.push(DoctorCollision {
                kind: DoctorCollisionKind::NormalizedSummaryCollision,
                key,
                paths,
            });
        }
    }
    collisions.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
    collisions
}

fn apply_collision_advisories(entries: &mut [DoctorEntry], collisions: &[DoctorCollision]) {
    for collision in collisions {
        let code = match collision.kind {
            DoctorCollisionKind::TraitIdCollision => "doctor.trait-id-collision",
            DoctorCollisionKind::NormalizedSummaryCollision => "doctor.summary-collision",
        };
        let subject = match collision.kind {
            DoctorCollisionKind::TraitIdCollision => "canonical trait ID",
            DoctorCollisionKind::NormalizedSummaryCollision => "normalized summary",
        };
        for path in &collision.paths {
            let others: Vec<String> = collision
                .paths
                .iter()
                .filter(|other| *other != path)
                .cloned()
                .collect();
            if others.is_empty() {
                continue;
            }
            if let Some(entry) = entries.iter_mut().find(|entry| &entry.path == path) {
                entry.advisories.push(DoctorAdvisory {
                    code: code.to_string(),
                    message: format!(
                        "{subject} {:?} also derived from: {}",
                        collision.key,
                        others.join(", ")
                    ),
                });
            }
        }
    }
}

fn next_action_for(root: &str, entry: &DoctorEntry) -> DoctorNextAction {
    let filename = entry.path.rsplit('/').next().unwrap_or(entry.path.as_str());
    if filename != "SKILL.md" {
        return DoctorNextAction {
            path: entry.path.clone(),
            command: None,
            caveat: Some(format!(
                "import currently requires a SKILL.md-named source; {} must be renamed or \
                 converted to SKILL.md (not symlinked — import rejects symlinked sources) \
                 before `ctx traits import` will accept it",
                entry.path
            )),
        };
    }

    let source_path = if entry.source_is_directory {
        match entry.path.rsplit_once('/') {
            Some((dir, _)) => join_root_relative(root, dir),
            None => root.to_string(),
        }
    } else {
        join_root_relative(root, &entry.path)
    };

    DoctorNextAction {
        path: entry.path.clone(),
        command: Some(format!(
            "ctx traits import --source {}",
            shell_quote_path(&source_path)
        )),
        caveat: None,
    }
}

/// Single-quote a path for safe inclusion in a POSIX shell command line,
/// escaping any embedded single quotes.
fn shell_quote_path(path: &str) -> String {
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('\'');
    for ch in path.chars() {
        if ch == '\'' {
            quoted.push_str("'\"'\"'");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// Join a doctor root display path with a root-relative candidate path into
/// one path a caller could actually pass to `--source` from the same working
/// directory doctor was invoked from.
fn join_root_relative(root: &str, relative_path: &str) -> String {
    if relative_path.is_empty() {
        return if root.is_empty() {
            ".".to_string()
        } else {
            root.to_string()
        };
    }
    if root.is_empty() || root == "." {
        relative_path.to_string()
    } else if let Some(stripped) = root.strip_suffix('/') {
        format!("{stripped}/{relative_path}")
    } else {
        format!("{root}/{relative_path}")
    }
}
