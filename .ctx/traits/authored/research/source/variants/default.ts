// research-default: plan a typed, cardinality-gated stream list, research
// each stream serially, triangulate, then grind a single-reviewer loop until
// approved — then commit. The plan's cardinality (2-4 streams) is a
// deterministic gate on the typed slot, never an agent judgment.
//
// Deviation from the draft (owner-triage item, 0153): the draft specified a
// runtime for-each over the stream plan (`packages/cdk/src/sequence.ts:397-
// 405`, `maxItems` as the structural upper-bound guarantee). The functional
// `slot.forEach` sugar (`packages/cdk/src/functional/registrars.ts`) mints
// its per-item slot as `slot.any(...)` unconditionally, and
// `modules/core/src/trait/procedure/validate.rs:2133` rejects any for-each
// whose item schema is `schema:any` — so a typed for-each is not reachable
// through the functional layer as shipped. Streams are researched serially
// within one worker turn instead (still cited, still one stream-finding per
// planned stream, same order); the plan's own cardinality gate is what
// bounds it. Fixing the functional for-each item-schema gap is out of this
// task's scope.
import { deriveParkReportStep, familyCommitTail, reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import { condition, flow, input, procedure, variant } from "@ctx-traits/cdk";

import { FAMILY_BEHAVIOR, FAMILY_INTENT } from "../core.ts";
import {
    citationStyle,
    commitOutput,
    commitReport,
    deriveReportPathStep,
    deriveTopicSlugStep,
    findings,
    outputDir,
    parkReport,
    parkReportPort,
    reportPath,
    reportPathPort,
    researchReportPort,
    researchStandards,
    sourceQualityGuide,
    streamPlan,
    topic,
    topicSlug,
    triangulation,
    verdict1,
    workSummary,
} from "../data.ts";

const smart = reviewerRole("smart-1", "Plans the research streams and is the sole reviewer of the delivered research.", "Planning and reviewer role.");
const worker = workerRole("worker", "Researches each stream, triangulates, drafts the report, and applies reviewer fixes.");
const scribe = scribeRole("scribe", "Writes the commit message for the completed research from the plan and verdict");

const streamCountValid = condition.any([
    condition.count(streamPlan).equals(2),
    condition.count(streamPlan).equals(3),
    condition.count(streamPlan).equals(4),
]);

export default variant({
    name: "Research (Default)",
    summary:
        "Surveyed dogfood research procedure: plan a cardinality-gated set of non-overlapping streams, research each serially, triangulate, then a bounded reviewer loop — then commit.",
    metadata: { tag: ["dogfood", "research", "review", "multi-agent"] },
    behavior: FAMILY_BEHAVIOR,
    intent: FAMILY_INTENT,
    resource: [researchStandards, sourceQualityGuide, citationStyle],
    port: [topic, outputDir, commitReport, parkReportPort, researchReportPort, reportPathPort],
    procedure: procedure.from(
        {
            description:
                "Research one topic end to end: plan 2-4 non-overlapping streams, research each serially, triangulate the findings, then a bounded single-reviewer refinement loop, then commit report.md, bibliography.md, and evidence.csv.",
        },
        () => {
            deriveTopicSlugStep();
            deriveReportPathStep("report.md");

            flow.loop("Planning", (loop) => {
                loop.maxIterations(3, { onExhausted: "abort" });

                smart.prompt("Plan the streams", {
                    input: input.prompt`
                        Plan two to four non-overlapping research streams for ${topic}. Each stream gets a stable id, a complete non-overlapping focus question, and kind "primary".
                        This is the run's one and only executable stream list, consumed directly by serial per-stream research — do not propose more than four.`,
                    output: streamPlan,
                    include: [streamPlan.optional()],
                });

                flow.until(streamCountValid);
            });

            worker.prompt("Research the streams", {
                input: input.prompt`
                    Research every stream in the plan ${streamPlan} for the topic ${topic}, one stream at a time and in the order planned — never merge two streams' evidence together.
                    Apply ${researchStandards}: cite every factual claim, rate sources per the canonical A-E scale (${sourceQualityGuide}), format citations per ${citationStyle}, and note any counterevidence encountered.
                    Return exactly one stream-finding per planned stream (same order, same ids): the stream's id and its cited findings summary. Write nothing to disk yet — this is evidence gathering only.`,
                output: findings,
            });

            worker.prompt("Triangulate the findings", {
                input: input.prompt`
                    Triangulate the researched findings ${findings} for ${topic}: where independent streams agree, where they conflict, source-quality ratings, and remaining gaps.
                    Return the triangulation.`,
                output: triangulation,
            });

            flow.loop("Building", (loop) => {
                loop.maxIterations(6, { onExhausted: "abort" });

                worker.prompt("Building Produce", {
                    input: input.prompt`
                        Produce the research delivery for ${topic} from the plan ${streamPlan}, findings ${findings}, and triangulation ${triangulation}. Write under ${outputDir}/${topicSlug}/ (a flat directory — no numbered subfolders, no manifests): report.md, bibliography.md, and evidence.csv (claim, source, rating, url).
                        No reviewer verdict attached means this is round 1: implement the full delivery. On every later round a verdict IS attached — fix every blocker it names.
                        Return a work summary: what was written, how citations were verified, open concerns.`,
                    output: workSummary,
                    include: [verdict1.optional(), workSummary.optional()],
                });

                smart.prompt("Building Review", {
                    input: input.prompt`
                        Review the research delivered for ${topic} against the plan ${streamPlan} and triangulation ${triangulation}. Work summary: ${workSummary}.
                        A BLOCKER is an uncited factual claim, a plan stream left uncovered, an unaddressed cross-stream contradiction, a source below C-rating used as sole support for a critical claim, or a file written outside the flat ${outputDir}/${topicSlug}/ layout. Everything else is advisory.
                        Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                        Set status to revise while any blocker remains, approved when none do.`,
                    output: verdict1,
                    include: [verdict1.optional()],
                });

                deriveParkReportStep(verdict1, { parkReportSlot: parkReport });

                flow.until(condition.equals(verdict1.status, "approved"));
            });

            familyCommitTail({
                id: "shipping",
                scribe: {
                    agent: scribe,
                    text: input.prompt`
                        The review for the research on ${topic} has ended approved and the work is being committed.
                        Write a concise commit message from the work summary ${workSummary} and the verdict ${verdict1}: a short subject line naming the topic, then one paragraph on what was researched and delivered at ${reportPath}.
                        Return exactly that message. Do not run git commands and do not write files.`,
                },
                receipt: commitOutput,
            });
        },
    ),
});
