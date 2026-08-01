import {
    agent,
    condition,
    operation,
    port,
    procedure,
    prompt,
    resource,
    schema,
    sequence,
    slot,
    trait,
} from "@ctx-traits/cdk";

const researchStandards = resource.file("research-standards", {
    path: "resources/research-standards.md",
    hint: "Canonical research process, citation, source-rating, verification, quality-gate, output-layout, and Mermaid guidance distilled from the source system.",
    trigger: "on-demand",
});
const qualityRubric = resource.file("quality-rubric", {
    path: "resources/quality-rubric.md",
    hint: "Reviewer guidance mapped onto the quality-verdict schema's canonical dimensions.",
    trigger: "on-demand",
});
const sourceQualityGuide = resource.file("source-quality-guide", {
    path: "resources/source-quality-guide.md",
    hint: "Domain examples for applying the canonical A-E source-quality scale.",
    trigger: "on-demand",
});
const citationStyle = resource.file("citation-style", {
    path: "resources/citation-style.md",
    hint: "Source-type citation formats and verification rules.",
    trigger: "on-demand",
});
const evidenceTableTemplate = resource.file("evidence-table-template", {
    path: "resources/evidence-table-template.csv",
    hint: "Verbatim CSV template for claim-to-source evidence tracking.",
    trigger: "on-demand",
});
const researchBriefTemplate = resource.file("research-brief-template", {
    path: "resources/research-brief-template.md",
    hint: "Research-plan scaffold with the runtime-native output tree.",
    trigger: "on-demand",
});
const researchReadmeTemplate = resource.file("research-readme-template", {
    path: "resources/research-readme-template.md",
    hint: "Navigation README template for a completed research campaign.",
    trigger: "on-demand",
});
const sourceNoteTemplate = resource.file("source-note-template", {
    path: "resources/source-note-template.md",
    hint: "Verbatim per-source provenance, credibility, evidence, and limitation template.",
    trigger: "on-demand",
});

const qualityLevel = schema.enum(
    "research-quality-level",
    ["basic", "standard", "rigorous"],
    { description: "Requested evidence-quality level; standard is the default." },
);
const depthLevel = schema.enum(
    "research-depth-level",
    ["basic", "standard", "rigorous"],
    { description: "Requested research depth; standard is the default." },
);
const researchStream = schema.object(
    "research-stream",
    {
        id: schema.field(schema.text(), {
            description: "Unique stable stream identifier.",
        }),
        focus: schema.field(schema.text(), {
            description: "Complete research question and scope assigned to this stream.",
        }),
        target: schema.field(schema.text(), {
            description: "Relative findings path, normally under 03_findings/.",
        }),
        "min-citations": schema.field(schema.integer(), {
            description: "Minimum cited sources expected from this stream.",
        }),
    },
    {
        description:
            "One non-overlapping serial research stream. Runtime status, attempts, heartbeats, and completion markers are deliberately absent.",
    },
);
const deliverable = schema.object(
    "research-deliverable",
    {
        id: schema.field(schema.text(), {
            description: "Unique deliverable identifier.",
        }),
        category: schema.field(
            schema.oneOf("plan", "summary", "report", "findings", "sources", "visuals", "metadata"),
            { description: "Output-tree category for this deliverable." },
        ),
        target: schema.field(schema.text(), {
            description: "Relative output path under research/<topic>/.",
        }),
        "min-bytes": schema.field(schema.integer(), {
            description: "Minimum accepted file size checked by independent review.",
        }),
        "min-citations": schema.field(schema.integer(), {
            description: "Minimum accepted citation count checked by independent review.",
        }),
        required: schema.field(schema.boolean(), {
            description: "Whether final delivery requires this artifact.",
        }),
        comment: schema.field(schema.text(), {
            required: false,
            description: "Optional planning note.",
        }),
    },
    {
        description:
            "One expected research artifact. Completion markers are omitted because typed step outputs and the run ledger own completion.",
    },
);
const qualityCriteria = schema.object(
    "research-quality-criteria",
    {
        "min-total-sources": schema.field(schema.integer(), {
            description: "Minimum source count for the campaign.",
        }),
        "target-a-b-rated-percent": schema.field(schema.number(), {
            description: "Target percentage of A- or B-rated sources.",
        }),
        "max-c-rated-percent": schema.field(schema.number(), {
            description: "Maximum percentage of C-rated sources.",
        }),
        "require-multiple-corroboration": schema.field(schema.boolean(), {
            description: "Whether critical claims require independent corroboration.",
        }),
        "citation-format": schema.field(schema.text(), {
            description: "Citation format selected for the campaign.",
        }),
    },
    { description: "Campaign-wide evidence and citation thresholds." },
);
const deliverablesManifest = schema.object(
    "deliverables-manifest",
    {
        topic: schema.field(schema.text(), {
            description: "Kebab-case campaign topic identifier.",
        }),
        "quality-target": schema.field(qualityLevel, {
            description: "Selected evidence-quality target.",
        }),
        "depth-level": schema.field(depthLevel, {
            description: "Selected research depth.",
        }),
        created: schema.field(schema.text(), {
            description: "ISO 8601 UTC creation timestamp.",
        }),
        deliverables: schema.field(schema.list(deliverable), {
            description: "Ordered expected artifacts.",
        }),
        "quality-criteria": schema.field(qualityCriteria, {
            description: "Campaign-wide evidence thresholds.",
        }),
    },
    { description: "Expected outputs and evidence thresholds for one research campaign." },
);

const qualityDimensionScore = schema.object(
    "quality-dimension-score",
    {
        score: schema.field(schema.rating(0, 3), {
            description: "Dimension score from 0 (not met) through 3 (fully met).",
        }),
        max: schema.field(schema.rating(3, 3), {
            description: "Maximum dimension score; fixed at 3.",
        }),
        notes: schema.field(schema.text(), {
            description: "Evidence and rationale for the score.",
        }),
    },
    { description: "One canonical quality dimension score." },
);
const qualityDimensions = schema.object(
    "quality-dimensions",
    {
        "citation-density": schema.field(qualityDimensionScore),
        "source-quality": schema.field(qualityDimensionScore),
        "claim-verification": schema.field(qualityDimensionScore),
        completeness: schema.field(qualityDimensionScore),
        coherence: schema.field(qualityDimensionScore),
        clarity: schema.field(qualityDimensionScore),
        depth: schema.field(qualityDimensionScore),
        actionability: schema.field(qualityDimensionScore),
    },
    {
        description:
            "The eight canonical dimensions from references/json-schemas.md used by this procedure's quality contract.",
    },
);
const qualityReview = schema.object(
    "quality-review",
    {
        completed: schema.field(schema.boolean(), {
            description: "Whether the independent review completed.",
        }),
        score: schema.field(schema.number(), {
            description: "Reviewer's normalized 0-10 score.",
        }),
        recommendations: schema.field(schema.list(schema.text()), {
            description: "Concrete corrections or improvements.",
        }),
    },
    { description: "Independent review evidence attached to a quality verdict." },
);
const qualityVerdict = schema.object(
    "quality-verdict",
    {
        "overall-score": schema.field(schema.number(), {
            description: "Dimension average multiplied by 3.33, on a 0-10 scale.",
        }),
        "target-score": schema.field(schema.number(), {
            description: "Required normalized score: 7.0 for basic, 8.5 for standard, or 9.2 for rigorous.",
        }),
        dimensions: schema.field(qualityDimensions),
        review: schema.field(qualityReview),
        "assessed-at": schema.field(schema.text(), {
            description: "ISO 8601 UTC assessment timestamp.",
        }),
        "assessed-by": schema.field(
            schema.oneOf("review-capability", "human", "self-review", "automated-gate"),
            { description: "Kind of assessor that produced the verdict." },
        ),
    },
    { description: "Typed 0-10 quality assessment used by review and guard steps." },
);

const researchFinding = schema.object(
    "research-finding",
    {
        "stream-id": schema.field(schema.text(), {
            description: "The research stream that produced this finding.",
        }),
        target: schema.field(schema.text(), {
            description: "Relative findings file written for the stream.",
        }),
        summary: schema.field(schema.text(), {
            description: "Concise summary of the stream's evidence-backed findings.",
        }),
        "citation-count": schema.field(schema.integer(), {
            description: "Number of citations in the written finding.",
        }),
        "source-types": schema.field(schema.list(schema.text()), {
            description: "Independent source types represented in the finding.",
        }),
        contradictions: schema.field(schema.list(schema.text()), {
            description: "Contradictions or unresolved evidence gaps found by the stream.",
        }),
    },
    { description: "One stream's durable finding receipt." },
);
const verificationVerdict = schema.object(
    "verification-verdict",
    {
        status: schema.field(schema.oneOf("approved", "revise"), {
            description: "Approved only when every critical claim is supported or removed.",
        }),
        issues: schema.field(schema.list(schema.text()), {
            description: "Unsupported claims, citation defects, contradictions, or missing deliverables.",
        }),
        notes: schema.field(schema.text(), {
            description: "Chain-of-Verification evidence and reviewer rationale.",
        }),
    },
    { description: "One Chain-of-Verification review verdict." },
);

const probe = agent("probe", {
    description:
        "Decomposes the topic into research streams and gathers targeted evidence with web-capable tools.",
    summary: "Fast research planning and evidence-probe role.",
    system:
        "Use web retrieval when available, cite inspected sources, distinguish evidence from inference, and never invent access or citations.",
});
const streamWorker = agent("worker", {
    description:
        "Researches one typed stream at a time, writes only its declared findings target, and returns a structured finding receipt.",
    summary: "Serial evidence-collection role.",
    system:
        "Follow the stream contract exactly, preserve source provenance, search for counterevidence, and write no runtime task or heartbeat files.",
});
const synthesis = agent("synthesis", {
    description:
        "Triangulates streams, assesses quality, synthesizes deliverables, and applies reviewer corrections.",
    summary: "Research synthesis and correction role.",
    system:
        "Prefer supportable conclusions over completeness theater; expose contradictions, uncertainty, and evidence gaps plainly.",
});
const independentReviewer = agent("reviewer", {
    description:
        "Independently verifies critical claims, deliverables, contradictions, and citation support in a fresh review session.",
    summary: "Independent research-verification role.",
    system:
        "Do not trust the synthesis summary. Inspect the written research and cited sources, and approve only supportable, complete work.",
});

const topic = port.input.text({
    id: "topic",
    description: "Research topic or question supplied by the just research recipe.",
});
const quality = port.input.of(qualityLevel, {
    id: "quality",
    optional: true,
    description: "Optional evidence-quality level (basic, standard, or rigorous). Defaults to standard when omitted.",
});
const depth = port.input.of(depthLevel, {
    id: "depth",
    optional: true,
    description: "Optional research-depth level (basic, standard, or rigorous). Defaults to standard when omitted.",
});

const topicSlug = slot.text({
    id: "topic-slug",
    description: "Kebab-case campaign directory name derived from the natural-language topic.",
});
const streamList = slot.array(researchStream, {
    id: "streams",
    description:
        "The executable stream collection consumed directly by serial for-each: three to six non-overlapping streams.",
});
const manifestStreamList = slot.array(researchStream, {
    id: "manifest-streams",
    description:
        "Independently authored comparison list of three to six non-overlapping streams, used only by the plan cardinality gate. Not consumed by execution; until P432 lands count-to-count equality, its role is a second independent 3-6 count proof rather than a duplicate-detection check.",
});
const deliverablesManifestSlot = slot({
    id: "deliverables-manifest",
    schema: deliverablesManifest,
    description: "Typed output and evidence contract for the campaign.",
});
const planReceipt = slot.text({
    id: "plan-receipt",
    description: "Paths of the plan and manifest files written from the typed plan.",
});
const currentStream = slot({
    id: "current-stream",
    schema: researchStream,
    description: "Current serial for-each stream binding.",
});
const findings = slot.array(researchFinding, {
    id: "findings",
    description: "Ordered receipts from completed research streams.",
});
const triangulation = slot.text({
    id: "triangulation",
    description: "Cross-stream contradiction, corroboration, source-quality, and gap analysis.",
});
const qualityVerdictSlot = slot({
    id: "quality-verdict",
    schema: qualityVerdict,
    description: "Canonical quality verdict for current evidence.",
});
const qualityScore = slot.number({
    id: "quality-score",
    description:
        "Numeric projection of quality-verdict.overall-score, emitted with the verdict because ordered object-field guards are not supported.",
});
const synthesisDraft = slot.text({
    id: "synthesis-draft",
    description: "Current synthesis receipt and paths, revised in place by the verification loop.",
});
const verificationVerdictSlot = slot({
    id: "verification-verdict",
    schema: verificationVerdict,
    description: "Latest bounded Chain-of-Verification verdict.",
});
const finalReviewVerdictSlot = slot({
    id: "final-review-verdict",
    schema: verificationVerdict,
    description: "Independent final review of the completed research package.",
});
const deliveryReport = slot.text({
    id: "delivery-report",
    description: "Final campaign path, quality score, limitations, and deliverable summary.",
});
const researchReport = port.output.text({
    id: "research-report",
    title: "Research Delivery",
    description: "Final deep-research delivery report and output path.",
    value: deliveryReport,
    format: "markdown",
});

const planStep = sequence.prompt("plan", {
    title: "Plan typed streams and deliverables",
    agent: probe,
    input: [quality, depth],
    text: prompt.text`
        Plan deep research for ${topic} without asking clarification questions.
        Read optional port:quality and port:depth from Resolved input values. If either is absent, treat it as unavailable and use the middle level standard for that setting.
        Apply ${researchStandards}, ${qualityRubric}, and ${sourceQualityGuide}.
        Produce exactly four typed outputs:
        1. topic-slug: a safe kebab-case campaign directory name with no slashes or traversal segments.
        2. streams: three to six non-overlapping research streams with id, focus, target, and min-citations; this is the campaign's one and only executable stream collection, consumed directly by serial for-each.
        3. manifest-streams: an independently authored three to six non-overlapping research streams with id, focus, target, and min-citations, used only by the cardinality guard.
        4. deliverables-manifest: topic-slug, quality-target, depth-level, ISO timestamp, runtime-native deliverables, and quality criteria.
        Copy supplied quality and depth exactly; never infer a different level. Do not include status, attempts, heartbeats, result markers, done-marker, locks, or logs.`,
    output: [topicSlug, streamList, manifestStreamList, deliverablesManifestSlot],
});
function countInRange(slotHandle) {
    return condition.any([
        condition.count(slotHandle).equals(3),
        condition.count(slotHandle).equals(4),
        condition.count(slotHandle).equals(5),
        condition.count(slotHandle).equals(6),
    ]);
}
const streamCountValid = condition.all([countInRange(streamList), countInRange(manifestStreamList)]);
const cardinalityCheckReceipt = slot.text({
    id: "cardinality-check-receipt",
    description: "Deterministic checkpoint marker for the stream-count cardinality guard.",
});
const cardinalityCheckStep = sequence.command({
    id: "cardinality-check",
    title: "Deterministic checkpoint for the cardinality guard",
    cmd: "true",
    output: cardinalityCheckReceipt,
});
const planGateStep = sequence.loop("plan-gate", {
    title: "Guard stream-count cardinality before materialization",
    sequence: sequence.linear("plan-cardinality-guard", [cardinalityCheckStep]),
    until: streamCountValid,
    iterations: 1,
    onExhausted: "block",
});
const writePlanStep = sequence.prompt("write-plan", {
    title: "Write the research plan",
    agent: probe,
    text: prompt.text`
        Materialize the approved plan for ${topic} under the safe root research/${topicSlug}/.
        Create the runtime-native 00_plan through 06_metadata directory structure. Fill ${researchBriefTemplate} with the approved ${topic}, ${streamList}, and ${deliverablesManifestSlot} and write the completed brief to 00_plan/00_research_plan.md; do not invent a parallel plan format. Write ${deliverablesManifestSlot} to 06_metadata/deliverables/manifest.json.
        Initialize 06_metadata/evidence_table.csv from ${evidenceTableTemplate} without creating runtime task, heartbeat, result-marker, log, lock, or gate.ok files.
        Return the paths written.`,
    output: planReceipt,
});
const researchOneStream = sequence.prompt("research-stream", {
    title: "Research one evidence stream",
    agent: streamWorker,
    text: prompt.text`
        Research ${currentStream} for ${topic} under the contract ${deliverablesManifestSlot}.
        Use ${researchStandards}, ${sourceQualityGuide}, ${citationStyle}, and ${sourceNoteTemplate}.
        Inspect every cited source, prefer primary A-B evidence, seek independent counterevidence, and write only the stream's declared target under research/${topicSlug}/.
        Do not write task registries, heartbeats, result markers, journals, locks, logs, or files outside this campaign.
        Return one research-finding object with stream-id, target, summary, citation-count, source-types, and contradictions.`,
    output: operation.over(findings, operation.Append),
});
const researchStreamsStep = sequence.forEach("research-streams", {
    title: "Research streams serially",
    over: streamList,
    item: currentStream,
    sequence: sequence.linear("research-one-stream", [researchOneStream]),
    maxItems: 6,
    output: findings,
});
const triangulateStep = sequence.prompt("triangulate", {
    title: "Triangulate sources and contradictions",
    agent: synthesis,
    text: prompt.text`
        Triangulate ${findings} for ${topic} using ${researchStandards}, ${qualityRubric}, and ${sourceQualityGuide}.
        Cross-reference critical claims across independent source types, reconcile or expose contradictions, identify paywalled or inaccessible evidence, and perform targeted gap searches with web-capable tools when available.
        Update the bibliography, source ratings, evidence table, and a cross-verification finding under research/${topicSlug}/.
        Return a concise contradiction, corroboration, source-quality, and remaining-gap analysis.`,
    output: triangulation,
});
const assessQualityStep = sequence.prompt("assess-quality", {
    title: "Assess the numeric quality gate",
    agent: synthesis,
    text: prompt.text`
        Assess ${findings} and ${triangulation} against ${deliverablesManifestSlot}, ${researchStandards}, and ${qualityRubric}.
        Score the canonical eight dimensions from 0 to 3, calculate overall-score on the 0-10 scale, and set target-score from quality-target (basic=7.0, standard=8.5, rigorous=9.2).
        Return exactly two typed outputs: quality-verdict and quality-score. quality-score MUST equal quality-verdict.overall-score exactly; it is the numeric projection the runtime guard reads.
        Write the verdict to research/${topicSlug}/06_metadata/quality_verdict.json. Do not add a model-authored passed boolean.`,
    output: [qualityVerdictSlot, qualityScore],
});
const qualityPass = condition.any([
    condition.all([
        condition.fieldEquals(deliverablesManifestSlot, "quality-target", "basic"),
        condition.gte(qualityScore, 7.0),
    ]),
    condition.all([
        condition.fieldEquals(deliverablesManifestSlot, "quality-target", "standard"),
        condition.gte(qualityScore, 8.5),
    ]),
    condition.all([
        condition.fieldEquals(deliverablesManifestSlot, "quality-target", "rigorous"),
        condition.gte(qualityScore, 9.2),
    ]),
]);
const qualityGateStep = sequence.loop("quality-gate", {
    title: "Assess and gate evidence quality",
    sequence: sequence.linear("assess-evidence-quality", [assessQualityStep]),
    until: qualityPass,
    stopIf: condition.not(qualityPass),
    iterations: 1,
    onExhausted: "block",
});

const synthesizeStep = sequence.prompt("synthesize", {
    title: "Synthesize the research deliverables",
    agent: synthesis,
    text: prompt.text`
        Synthesize ${findings}, ${triangulation}, and ${qualityVerdictSlot} for ${topic} using ${researchStandards}, ${citationStyle}, ${researchReadmeTemplate}, and ${deliverablesManifestSlot}.
        Write the required executive summary, full report, bibliography, source ratings, methodology, useful tables or safely quoted Mermaid diagrams, and campaign README under research/${topicSlug}/.
        Every factual claim must be cited. Preserve counterevidence, uncertainty, inaccessible-source caveats, and the quality verdict. Return the paths written and a concise synthesis summary.`,
    output: synthesisDraft,
});
const verifyStep = sequence.prompt("verify", {
    title: "Verify critical claims and deliverables",
    agent: independentReviewer,
    text: prompt.text`
        Independently verify the current research ${synthesisDraft} for ${topic} using Chain-of-Verification in ${researchStandards}, the contract ${deliverablesManifestSlot}, and the source files under research/${topicSlug}/.
        Check critical claims against inspected sources, citation accessibility and support, contradictions, required paths, minimum size/citation expectations, banned markers, internal links, and promised diagrams.
        Return status approved only when every critical claim is supported or removed and all required deliverables are coherent; otherwise return revise with concrete issues.`,
    output: verificationVerdictSlot,
});
const reviseStep = sequence.prompt("revise", {
    title: "Apply verification corrections",
    agent: synthesis,
    text: prompt.text`
        Apply ${verificationVerdictSlot} to the current research ${synthesisDraft} under research/${topicSlug}/.
        Correct, qualify, or remove unsupported claims; repair citation and deliverable defects; preserve valid evidence and structure. If status is approved, make no changes and return the existing synthesis receipt.
        Return the complete updated synthesis receipt and paths.`,
    output: synthesisDraft,
});
const verificationLoop = sequence.loop("verification-loop", {
    title: "Bounded Chain-of-Verification",
    sequence: sequence.linear("verify-and-revise", [verifyStep, reviseStep]),
    until: condition.fieldEquals(verificationVerdictSlot, "status", "approved"),
    iterations: 3,
    onExhausted: "block",
});
const finalReviewStep = sequence.prompt("final-review", {
    title: "Perform independent final review",
    agent: independentReviewer,
    text: prompt.text`
        Perform a final review of research/${topicSlug}/ for ${topic} against ${deliverablesManifestSlot}.
        Inspect every required path and minimum size/citation expectation, then inspect the executive summary, report, findings, bibliography, source ratings, methodology, quality verdict, contradictions, evidence table, internal links, and unfinished markers.
        Return approved only when the package is coherent, decision-useful, appropriately caveated, and faithful to inspected sources; otherwise return revise with concrete issues.`,
    output: finalReviewVerdictSlot,
});
const finalReviewReviseStep = sequence.prompt("final-review-revise", {
    title: "Apply final-review corrections",
    agent: synthesis,
    text: prompt.text`
        Apply ${finalReviewVerdictSlot} to the completed research package under research/${topicSlug}/.
        Correct, qualify, or remove unsupported claims; repair citation, deliverable, or packaging defects; preserve valid evidence and structure. If status is approved, make no changes and return the existing synthesis receipt.
        Return the complete updated synthesis receipt and paths.`,
    output: synthesisDraft,
});
const finalReviewGateStep = sequence.loop("final-review-gate", {
    title: "Gate packaging on final review",
    sequence: sequence.linear("review-and-revise-final", [finalReviewStep, finalReviewReviseStep]),
    until: condition.fieldEquals(finalReviewVerdictSlot, "status", "approved"),
    iterations: 3,
    onExhausted: "block",
});
const deliverStep = sequence.prompt("deliver", {
    title: "Report the research delivery",
    agent: synthesis,
    text: prompt.text`
        Report the completed research for ${topic} from ${synthesisDraft}, ${qualityVerdictSlot}, and ${finalReviewVerdictSlot}. The numeric quality gate, bounded verification loop, and independent final review all passed before this step became ready.
        Return the campaign path, required deliverables, normalized quality score and target, source/citation summary, known limitations, and next steps.`,
    output: deliveryReport,
});

export default trait("deep-research", {
    version: "0.2.0",
    name: "Deep Research",
    summary:
        "Typed deep-research contracts and standards for evidence collection, triangulation, synthesis, and verification.",
    metadata: {
        tag: ["research", "evidence", "citations", "verification"],
    },
    port: researchReport,
    resource: [
        researchStandards,
        qualityRubric,
        sourceQualityGuide,
        citationStyle,
        evidenceTableTemplate,
        researchBriefTemplate,
        researchReadmeTemplate,
        sourceNoteTemplate,
    ],
    procedure: procedure({
        description:
            "Plan typed research contracts from topic plus optional quality and depth, research streams serially, triangulate and gate evidence, synthesize, verify in a bounded loop, and deliver a structured research campaign.",
        sequence: [
            planStep,
            planGateStep,
            writePlanStep,
            researchStreamsStep,
            triangulateStep,
            qualityGateStep,
            synthesizeStep,
            verificationLoop,
            finalReviewGateStep,
            deliverStep,
        ],
    }),
});
