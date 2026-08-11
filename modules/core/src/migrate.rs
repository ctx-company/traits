//! Version-to-version trait migration: mechanical rewrites composed across a
//! registry of schema-version steps, with a designed (not yet implemented)
//! seam for constructs no mechanical rule can rewrite.
//!
//! **Multi-version strategy (decided).** Mechanical steps compose: migrating
//! N -> M walks every registry step on the path and applies them in order to
//! one in-memory document. A future assisted pass runs at most ONCE, against
//! the final target schema, over the original author's prose captured before
//! any mechanical rewrite — never over an intermediate mechanical or model
//! output. Composing assisted passes would make pass 2 reason about pass 1's
//! output rather than the author's intent, which is the actual failure mode
//! (see `.internal/tasks/0028-version-migration-mechanical-and-assisted.md`,
//! "the hard case: skipping versions", option 2).
//!
//! Planning is pure: no filesystem or process IO happens in this module.
//! `toml_edit` preserves untouched byte spans verbatim, so a mechanical
//! migration is diffable and re-running it on its own output is a no-op.

use toml_edit::DocumentMut;

use crate::digest::{Digest, canonical_digest, source_digest};
use crate::encoding::{Encoding, decode_trait};
use crate::r#trait::is_schema_version_supported;

/// Migration planning errors. Every variant is a refusal: a migration never
/// writes a best-effort or partially-rewritten document.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("migration step {step} refused: {reason}")]
    Refused { step: String, reason: String },
}

impl Error {
    fn refused(step: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Refused {
            step: step.into(),
            reason: reason.into(),
        }
    }
}

/// One field or construct a mechanical step could not rewrite, deferred to a
/// future assisted pass. Carries the pre-mechanical original text so an
/// assisted pass reasons about the author's own prose, not the mechanical
/// engine's partial output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistedItem {
    pub step: &'static str,
    pub excerpt: String,
    pub reason: String,
}

/// One rewrite a step performed, recorded for the plan report / diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    pub description: String,
}

/// What a step produced: the rewrites it made, plus any constructs it
/// declined to rewrite (the assisted seam).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepOutcome {
    pub rewrites: Vec<Rewrite>,
    pub assisted_needed: Vec<AssistedItem>,
}

impl StepOutcome {
    fn rewrite(description: impl Into<String>) -> Self {
        Self {
            rewrites: vec![Rewrite {
                description: description.into(),
            }],
            assisted_needed: Vec::new(),
        }
    }
}

/// One version-to-version mechanical step in the registry.
pub struct MigrationStep {
    pub from: &'static str,
    pub to: &'static str,
    pub rewrite: fn(&mut DocumentMut) -> Result<StepOutcome, Error>,
}

/// Ordered registry of mechanical migration steps, one entry per adjacent
/// schema-version pair. Order is the sole source of truth for version
/// adjacency — never derive it by parsing "0.2"/"0.3" as semver;
/// `SchemaVersion` is an opaque validated string
/// (`crate::r#trait::SchemaVersion`).
pub const MIGRATION_STEPS: &[MigrationStep] = &[
    MigrationStep {
        from: "0.2",
        to: "0.3",
        rewrite: bump_0_2_to_0_3,
    },
    MigrationStep {
        from: "0.3",
        to: "0.4",
        rewrite: rename_summary_to_description_0_3_to_0_4,
    },
];

/// The only 0.2 -> 0.3 rewrite: every 0.3-gated feature (`variant`,
/// presence-aware conditions, count-to-count comparisons, keep-guard count
/// staleness) is additive, so nothing valid under 0.2 needs changing beyond
/// the declared version.
fn bump_0_2_to_0_3(doc: &mut DocumentMut) -> Result<StepOutcome, Error> {
    doc["schema-version"] = toml_edit::value("0.3");
    Ok(StepOutcome::rewrite("schema-version: \"0.2\" -> \"0.3\""))
}

/// 0.3 -> 0.4: `description` becomes the required agent+user prose (task
/// 0165); the author's existing `summary` carries that text forward
/// unchanged. A document that already declares a distinct `description`
/// (authored ahead of the bump) is left alone — only `summary` is renamed
/// when no `description` key exists yet.
fn rename_summary_to_description_0_3_to_0_4(doc: &mut DocumentMut) -> Result<StepOutcome, Error> {
    doc["schema-version"] = toml_edit::value("0.4");
    if doc.contains_key("description") {
        return Ok(StepOutcome::rewrite("schema-version: \"0.3\" -> \"0.4\""));
    }
    let Some(summary) = doc.remove("summary") else {
        return Err(Error::refused(
            "rename-summary-to-description",
            "no summary field to rename and no description already present",
        ));
    };
    doc["description"] = summary;
    Ok(StepOutcome {
        rewrites: vec![
            Rewrite {
                description: "schema-version: \"0.3\" -> \"0.4\"".to_string(),
            },
            Rewrite {
                description: "summary -> description".to_string(),
            },
        ],
        assisted_needed: Vec::new(),
    })
}

/// A planned (or, once written by the caller, applied) migration.
#[derive(Debug, Clone)]
pub struct MigrationPlan {
    pub from: String,
    pub to: String,
    pub source_text: String,
    pub output_text: String,
    pub rewrites: Vec<Rewrite>,
    pub source_digest_before: Digest,
    pub source_digest_after: Digest,
    pub canonical_digest_before: Digest,
    pub canonical_digest_after: Digest,
    pub assisted_needed: Vec<AssistedItem>,
}

impl MigrationPlan {
    /// Whether applying this plan would change the source bytes at all.
    pub fn is_noop(&self) -> bool {
        self.source_text == self.output_text
    }
}

/// Plan a migration from the document's current `schema-version` to
/// `target`.
///
/// Pure planning: parses `source_text`, composes the registry path from the
/// current version to `target`, rewrites an in-memory `toml_edit` document,
/// then validates by round-trip through [`decode_trait`] under `target` —
/// refusing rather than returning a plausible-looking but undecodable
/// document. A nonempty `assisted_needed` on the returned plan blocks
/// `--apply`: assisted migration is a designed seam, not yet implemented.
pub fn plan_migration(source_text: &str, target: &str) -> Result<MigrationPlan, Error> {
    if !is_schema_version_supported(target) {
        return Err(Error::refused(
            "parse",
            format!("unsupported target schema-version {target:?}"),
        ));
    }

    let source_trait = decode_trait(Encoding::Toml, source_text)
        .map_err(|e| Error::refused("parse", format!("source failed to decode: {e}")))?;
    let from = source_trait.schema_version.as_str().to_string();
    let canonical_digest_before = canonical_digest(&source_trait)
        .map_err(|e| Error::refused("parse", format!("failed to digest source: {e}")))?;

    let mut doc: DocumentMut = source_text
        .parse()
        .map_err(|e| Error::refused("parse", format!("source is not valid TOML: {e}")))?;

    let path = registry_path(&from, target)?;

    let mut rewrites = Vec::new();
    let mut assisted_needed = Vec::new();
    for step in &path {
        let outcome = (step.rewrite)(&mut doc)?;
        rewrites.extend(outcome.rewrites);
        assisted_needed.extend(outcome.assisted_needed);
    }

    let output_text = doc.to_string();

    let migrated_trait = decode_trait(Encoding::Toml, &output_text).map_err(|e| {
        Error::refused(
            "round-trip",
            format!("migrated output failed to decode under target schema {target:?}: {e}"),
        )
    })?;
    if migrated_trait.schema_version.as_str() != target {
        return Err(Error::refused(
            "round-trip",
            format!(
                "migrated output decodes as schema-version {:?}, expected {target:?}",
                migrated_trait.schema_version.as_str()
            ),
        ));
    }
    let canonical_digest_after = canonical_digest(&migrated_trait)
        .map_err(|e| Error::refused("round-trip", format!("failed to digest output: {e}")))?;

    Ok(MigrationPlan {
        from,
        to: target.to_string(),
        source_digest_before: source_digest(source_text),
        source_digest_after: source_digest(&output_text),
        canonical_digest_before,
        canonical_digest_after,
        source_text: source_text.to_string(),
        output_text,
        rewrites,
        assisted_needed,
    })
}

/// Walk [`MIGRATION_STEPS`] to find the ordered chain of steps from `from`
/// to `to`. Refuses if either endpoint is unsupported or no registry path
/// connects them (including a `to` that is "behind" `from`).
fn registry_path(from: &str, to: &str) -> Result<Vec<&'static MigrationStep>, Error> {
    if from == to {
        return Ok(Vec::new());
    }
    if !is_schema_version_supported(from) {
        return Err(Error::refused(
            "parse",
            format!("unsupported source schema-version {from:?}"),
        ));
    }

    let mut path = Vec::new();
    let mut current = from;
    while current != to {
        let Some(step) = MIGRATION_STEPS.iter().find(|s| s.from == current) else {
            return Err(Error::refused(
                "plan",
                format!("no registry path from {from:?} to {to:?}"),
            ));
        };
        path.push(step);
        current = step.to;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V02: &str = r#"
schema-version = "0.2"
id = "example/demo"
version = "1.0.0"
name = "Demo"
summary = "A demo trait."
"#;

    const V03: &str = r#"
schema-version = "0.3"
id = "example/demo"
version = "1.0.0"
name = "Demo"
summary = "A demo trait."
"#;

    #[test]
    fn plans_a_pure_version_bump() {
        let plan = plan_migration(V02, "0.3").expect("0.2 -> 0.3 migrates");
        assert_eq!(plan.from, "0.2");
        assert_eq!(plan.to, "0.3");
        assert!(plan.assisted_needed.is_empty());
        assert_eq!(plan.rewrites.len(), 1);
        assert_ne!(plan.source_digest_before, plan.source_digest_after);
        assert_ne!(plan.canonical_digest_before, plan.canonical_digest_after);
    }

    #[test]
    fn diff_touches_only_schema_version_line() {
        let plan = plan_migration(V02, "0.3").expect("migrates");
        let before_lines: Vec<&str> = plan.source_text.lines().collect();
        let after_lines: Vec<&str> = plan.output_text.lines().collect();
        assert_eq!(before_lines.len(), after_lines.len());
        let changed: Vec<(&str, &str)> = before_lines
            .iter()
            .zip(after_lines.iter())
            .filter(|(a, b)| a != b)
            .map(|(a, b)| (*a, *b))
            .collect();
        assert_eq!(
            changed,
            vec![("schema-version = \"0.2\"", "schema-version = \"0.3\"")]
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let once = plan_migration(V02, "0.3").expect("migrates once");
        let twice = plan_migration(&once.output_text, "0.3").expect("migrates twice");
        assert!(twice.is_noop());
        assert_eq!(twice.output_text, once.output_text);
    }

    #[test]
    fn refuses_unsupported_target() {
        let err = plan_migration(V02, "9.9").expect_err("unsupported target refuses");
        assert!(matches!(err, Error::Refused { .. }));
    }

    #[test]
    fn refuses_unsupported_source() {
        let doc = V02.replace("0.2", "9.9");
        // Bypasses `decode_trait`'s own schema-version validation to reach
        // `plan_migration`'s own refusal path directly is not possible here
        // since decode_trait already refuses first — asserting that a
        // malformed/unsupported source is refused before any rewrite runs.
        let err = plan_migration(&doc, "0.3").expect_err("unsupported source refuses");
        assert!(matches!(err, Error::Refused { .. }));
    }

    #[test]
    fn refuses_missing_registry_path() {
        // No step exists from 0.3 back to 0.2.
        let v03 = V02.replace("0.2", "0.3");
        let err = plan_migration(&v03, "0.2").expect_err("backward migration refuses");
        assert!(matches!(err, Error::Refused { .. }));
    }

    #[test]
    fn same_version_target_is_a_noop_plan() {
        let plan = plan_migration(V02, "0.2").expect("same-version plans");
        assert!(plan.is_noop());
        assert!(plan.rewrites.is_empty());
    }

    #[test]
    fn synthetic_step_composition_and_span_preservation() {
        // Exercise a two-step chain end-to-end using the real registry's
        // 0.2 -> 0.3 step plus a local, test-only synthetic step function
        // (not registered in `MIGRATION_STEPS`) applied by hand in sequence,
        // proving that composing rewrites over one document preserves
        // untouched spans and that a step can refuse mid-chain.
        fn rename_summary_to_synopsis(doc: &mut DocumentMut) -> Result<StepOutcome, Error> {
            let Some(item) = doc.remove("summary") else {
                return Err(Error::refused("synthetic", "no summary field to rename"));
            };
            doc["synopsis"] = item;
            Ok(StepOutcome::rewrite("summary -> synopsis"))
        }

        let plan = plan_migration(V02, "0.3").expect("mechanical bump succeeds");
        let mut doc: DocumentMut = plan.output_text.parse().expect("re-parses");
        let untouched_id_line = doc
            .to_string()
            .lines()
            .find(|l| l.starts_with("id "))
            .map(str::to_string);

        let outcome = rename_summary_to_synopsis(&mut doc).expect("synthetic step applies");
        assert_eq!(outcome.rewrites.len(), 1);
        let after = doc.to_string();
        assert!(after.contains("synopsis"));
        assert!(!after.contains("\nsummary"));
        assert_eq!(
            after
                .lines()
                .find(|l| l.starts_with("id "))
                .map(str::to_string),
            untouched_id_line,
            "span for an untouched key must survive a second step verbatim"
        );

        // Refusal: renaming a field that no longer exists (already renamed).
        let err = rename_summary_to_synopsis(&mut doc).expect_err("second rename refuses");
        assert!(matches!(err, Error::Refused { .. }));
    }

    /// Release gate (task 0029): `SUPPORTED_SCHEMA_VERSIONS` must never gain
    /// a version without a corresponding, working `MIGRATION_STEPS` entry.
    mod release_gate {
        use super::*;
        use crate::r#trait::SUPPORTED_SCHEMA_VERSIONS;

        /// Minimal fixture text for a given supported schema version, used
        /// only to prove a registered step actually migrates. A new
        /// supported version with no arm here panics, forcing the author to
        /// add one.
        fn minimal_fixture(version: &str) -> &'static str {
            match version {
                "0.2" => V02,
                "0.3" => V03,
                other => panic!("add a minimal fixture for schema {other} (task 0029)"),
            }
        }

        #[test]
        fn every_supported_version_step_has_a_declared_migration() {
            assert_eq!(
                MIGRATION_STEPS.len(),
                SUPPORTED_SCHEMA_VERSIONS.len() - 1,
                "SUPPORTED_SCHEMA_VERSIONS and MIGRATION_STEPS have diverged: \
                 declare a MigrationStep for each adjacent pair; if nothing \
                 changed for authors, declare an explicit no-op step that \
                 only bumps schema-version (task 0029)"
            );
            for (i, window) in SUPPORTED_SCHEMA_VERSIONS.windows(2).enumerate() {
                let (from, to) = (window[0], window[1]);
                let step = &MIGRATION_STEPS[i];
                assert_eq!(
                    (step.from, step.to),
                    (from, to),
                    "declare a MigrationStep for {from} -> {to}; if nothing \
                     changed for authors, declare an explicit no-op step \
                     that only bumps schema-version (task 0029)"
                );
            }
        }

        #[test]
        fn every_declared_step_actually_migrates() {
            for step in MIGRATION_STEPS {
                let fixture = minimal_fixture(step.from);
                let plan = plan_migration(fixture, step.to)
                    .unwrap_or_else(|e| panic!("{} -> {} refused: {e}", step.from, step.to));
                assert_eq!(plan.to, step.to);
            }
        }
    }
}
