// Launch claim evidence definitions.
/// Launch claim evidence definitions.
use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::manifest::PackageStatus;
use crate::model_view::compile_model_view_with_evidence;
use crate::render::ExtendedRenderProfile;
use crate::resource_plan::plan_resource_inclusion;
use crate::response::{CapabilityReport, Warning};
use crate::r#trait::{PortDirection, Trait, TrustVerdict};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ClaimEvidenceMatrix {
    pub summary: String,
    pub rows: Vec<ClaimEvidenceRow>,
    pub blocked_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ClaimEvidenceRow {
    pub claim: String,
    pub phase: String,
    pub implementation_status: String,
    pub source_review_status: String,
    pub smoke_evidence: String,
    pub unsupported_capabilities: Vec<String>,
    pub allowed_wording: String,
}

pub fn claim_evidence_matrix() -> ClaimEvidenceMatrix {
    let rows = vec![
        claim_row(
            "package/version",
            "P1-P12, P51",
            "implemented",
            "source-approved",
            "canonical decode/check/render/export source paths are present",
            &[],
            "canonical trait packages carry typed identity and version metadata; package status and machine trust live outside the canonical document",
        ),
        claim_row(
            "import-lifecycle",
            "P59.1, P385",
            "implemented",
            "source-approved",
            "import always writes package status=draft and leaves machine trust unreviewed",
            &["automatic trust promotion"],
            "imports become unreviewed review candidates, not trusted runtime policy",
        ),
        claim_row(
            "audit",
            "P44-P51",
            "implemented",
            "source-approved",
            "hidden-content and advisory findings are surfaced by check/render paths",
            &["security certification", "complete secret detection"],
            "audit reports review findings and advisory risks; it is not a security certificate",
        ),
        claim_row(
            "check",
            "P45-P51",
            "implemented",
            "source-approved",
            "check combines validation, audit, resource, render, eval, and lock drift evidence",
            &["proof of model behavior"],
            "check reports deterministic source and artifact evidence; it does not prove model obedience",
        ),
        claim_row(
            "diff",
            "P48-P51",
            "implemented",
            "source-approved",
            "layer-aware diff reports missing/drift/current evidence",
            &["semantic proof of behavior"],
            "diff shows canonical/model-view/resource/policy/export evidence drift where available",
        ),
        claim_row(
            "model-view",
            "P50-P72",
            "implemented",
            "source-approved",
            "model-view digest is available through render/WASM ABI surfaces",
            &["claiming the model retained or obeyed all context"],
            "model-view previews deterministic model-visible text and digest evidence",
        ),
        claim_row(
            "render/export",
            "P51-P72",
            "implemented",
            "source-approved",
            "render/export profiles emit generated compatibility artifacts and warnings",
            &[
                "host-native enforcement",
                "dynamic injection for static hosts",
            ],
            "render/export produces reviewable host files with explicit semantic-loss warnings",
        ),
        claim_row(
            "resolve/explain",
            "P40-P58, P75",
            "implemented",
            "source-approved",
            "activation explain and budgeted resolve report selected and skipped candidates",
            &["retrieval as activation", "lifecycle bypass"],
            "resolve/explain reports why traits are selected, skipped, or gated",
        ),
        claim_row(
            "context budget",
            "P58, P75",
            "implemented",
            "source-approved",
            "resolve/pack accept explicit token budgets and report estimates",
            &[
                "exact billing tokens",
                "silent dropping of on-activation resources",
            ],
            "budget reports deterministic estimates and skip reasons, not exact provider billing",
        ),
        claim_row(
            "eval metadata",
            "P43-P44, P79",
            "implemented",
            "source-approved",
            "scenario/eval declarations and eval-result evidence are reportable",
            &[
                "future behavior proof",
                "provider calls during evidence generation",
            ],
            "evals are limited review metadata and deterministic checks, not behavior proof",
        ),
        claim_row(
            "host profiles",
            "P51-P72, P80",
            "implemented",
            "source-approved",
            "compatibility matrix lists profile paths, warnings, and fallback advice",
            &["replacement for host-native files"],
            "host profiles are compatibility targets with visible loss, not replacements",
        ),
        claim_row(
            "plugins",
            "P68-P72, P78",
            "scaffolded",
            "source-approved",
            "compatibility, policy, and subagent reports expose plugin scaffold capability plans and unsupported operations",
            &[
                "automatic install",
                "direct plugins command",
                "unsupported host hooks",
                "Markdown enforcement",
            ],
            "plugin scaffolds and capability plans are reportable; no direct plugins command or auto-install is claimed",
        ),
        runtime_claim_row(),
        claim_row(
            "OpenCode/MCP runtime UX",
            "Group 29-33 launch gate",
            "gated",
            "blocked-pending-runtime-family-approval",
            "MCP adapter and OpenCode scaffold paths expose runtime calls, but launch UX approval is still pending",
            &[
                "approved OpenCode production UX",
                "complete MCP resource/prompt coverage",
                "host-native enforcement",
            ],
            "OpenCode/MCP runtime UX is gated dogfood; cite only explicit unsupported-capability reports and source-reviewed adapter boundaries",
        ),
        claim_row(
            "CDK-first authoring",
            "Group 32-33 launch gate",
            "gated",
            "blocked-pending-runtime-family-approval",
            "CDK build and drift checks exist, but CDK-first launch positioning is still pending owner approval",
            &[
                "primary install path",
                "cold-machine proof",
                "provider-backed authoring",
            ],
            "CDK helpers can be described as local authoring scaffolds; do not make CDK-first launch claims until install and cold-machine gates close",
        ),
        claim_row(
            "procedure sequence control",
            "Group 28 P106-P111",
            "implemented",
            "source-approved",
            "reviewer-approved source behavior covers single-caller sequence-control mechanics: nested sequences, direct output ports, bounded loop guard exit, typed for-each, stop reasons, and slot revisions",
            &[
                "model-quality proof",
                "prompt-only Markdown loop enforcement",
                "unbounded autonomous completion",
                "multi-harness loop proof",
            ],
            "sequence-control mechanics are source-approved for single-caller runtime control; multi-harness proof remains gated by the runtime-family rows",
        ),
        claim_row(
            "llm-assisted authoring",
            "generate/compose/refine",
            "partial-scaffold",
            "not-claimed-for-launch",
            "deterministic command boundaries exist, but launch copy must not present provider-backed authoring as current proof",
            &[
                "provider availability",
                "model output trust",
                "automatic activation",
            ],
            "planned or unsupported-provider scaffold only unless a source-approved provider path is cited",
        ),
        claim_row(
            "lifecycle commands",
            "review/activate/create/audit",
            "target-product-design",
            "not-claimed-for-launch",
            "P73-P82 launch-safe demos use claim-gate/report surfaces instead of lifecycle command claims",
            &["automatic trust promotion", "unreviewed activation"],
            "future target workflow only unless the claim gate marks a command source-approved",
        ),
    ];
    let blocked_count = rows
        .iter()
        .filter(|row| {
            row.source_review_status.contains("blocked")
                || row.source_review_status.contains("gated")
        })
        .count();
    ClaimEvidenceMatrix {
        summary: "Launch copy must use allowed wording, preserve non-claims, and label unsupported or future command families as not claimed for launch.".to_string(),
        rows,
        blocked_count,
    }
}

/// Runtime posture derived from the reviewed claim-evidence matrix.
///
/// Runtime adapters use this instead of carrying their own launch-gating prose,
/// keeping the CLI and MCP surfaces aligned with the evidence report.
pub fn runtime_posture() -> (Warning, CapabilityReport) {
    let row = runtime_claim_row();
    let message = row.allowed_wording.clone();
    (
        Warning::new(
            crate::response::warning_code::RUNTIME_CLAIM_GATE,
            message.clone(),
        ),
        CapabilityReport::unsupported("runtime.launch-approval", message),
    )
}

/// The runtime claim is modeled once and projected into both the evidence matrix
/// and runtime envelopes, avoiding a fragile lookup by display text.
fn runtime_claim_row() -> ClaimEvidenceRow {
    claim_row(
        "multi-agent/multi-harness runtime",
        "Group 28-33 launch gate",
        "gated",
        "blocked-pending-runtime-family-approval",
        "drive/next/run-session surfaces exist, but launch approval for cross-harness writer/reviewer runtime is still owner-gated",
        &[
            "launch-approved multi-harness proof",
            "automatic harness trust",
            "model-quality proof",
        ],
        "runtime surfaces are available for controlled dogfood only; do not present multi-harness runtime as launch-approved until this row is unblocked",
    )
}

fn claim_row(
    claim: &str,
    phase: &str,
    implementation_status: &str,
    source_review_status: &str,
    smoke_evidence: &str,
    unsupported_capabilities: &[&str],
    allowed_wording: &str,
) -> ClaimEvidenceRow {
    ClaimEvidenceRow {
        claim: claim.to_string(),
        phase: phase.to_string(),
        implementation_status: implementation_status.to_string(),
        source_review_status: source_review_status.to_string(),
        smoke_evidence: smoke_evidence.to_string(),
        unsupported_capabilities: unsupported_capabilities
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        allowed_wording: allowed_wording.to_string(),
    }
}
