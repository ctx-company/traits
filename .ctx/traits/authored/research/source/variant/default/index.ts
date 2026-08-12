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
import { deriveParkReportStep, reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Default", {
    name: "Research (Default)",
    summary:
      "Surveyed dogfood research procedure: plan a cardinality-gated set of non-overlapping streams, research each serially, triangulate, then a bounded reviewer loop — then commit.",
    metadata: { tag: shared.metadata.fullTag },
    description:
      "Research one topic end to end: plan 2-4 non-overlapping streams, research each serially, triangulate the findings, then a bounded single-reviewer refinement loop, then commit report.md, bibliography.md, and evidence.csv.",
  });
  useBehavior(shared.behavior.family);
  useIntent(shared.intent.full);

  const smart = reviewerRole(
    "smart-1",
    "Plans the research streams and is the sole reviewer of the delivered research.",
    "Planning and reviewer role.",
  );
  const worker = workerRole(
    "worker",
    "Researches each stream, triangulates, drafts the report, and applies reviewer fixes.",
  );
  const scribe = scribeRole("scribe", "Writes the commit message for the completed research from the plan and verdict");

  const streamCountValid = condition.any([
    condition.count(shared.data.streamPlan).equals(2),
    condition.count(shared.data.streamPlan).equals(3),
    condition.count(shared.data.streamPlan).equals(4),
  ]);

  shared.stage.derive.deriveTopicSlugStep();
  shared.stage.derive.deriveReportPathStep("report.md");

  flow.loop("Planning", (loop) => {
    loop.maxIterations(3, { onExhausted: "abort" });

    smart.prompt("Plan the streams", {
      input: input.prompt`
                Plan two to four non-overlapping research streams for ${shared.data.topic}. Each stream gets a stable id, a complete non-overlapping focus question, and kind "primary".
                This is the run's one and only executable stream list, consumed directly by serial per-stream research — do not propose more than four.`,
      output: shared.data.streamPlan,
      include: [shared.data.streamPlan.optional()],
    });

    flow.until(streamCountValid);
  });

  worker.prompt("Research the streams", {
    input: input.prompt`
                Research every stream in the plan ${shared.data.streamPlan} for the topic ${shared.data.topic}, one stream at a time and in the order planned — never merge two streams' evidence together.
                Apply ${shared.resource.researchStandards}: cite every factual claim, rate sources per the canonical A-E scale (${shared.resource.sourceQualityGuide}), format citations per ${shared.resource.citationStyle}, and note any counterevidence encountered.
                Return exactly one stream-finding per planned stream (same order, same ids): the stream's id and its cited findings summary. Write nothing to disk yet — this is evidence gathering only.`,
    output: shared.data.findings,
  });

  worker.prompt("Triangulate the findings", {
    input: input.prompt`
                Triangulate the researched findings ${shared.data.findings} for ${shared.data.topic}: where independent streams agree, where they conflict, source-quality ratings, and remaining gaps.
                Return the triangulation.`,
    output: shared.data.triangulation,
  });

  flow.loop("Building", (loop) => {
    loop.maxIterations(6, { onExhausted: "abort" });

    worker.prompt("Building Produce", {
      input: input.prompt`
                    Produce the research delivery for ${shared.data.topic} from the plan ${shared.data.streamPlan}, findings ${shared.data.findings}, and triangulation ${shared.data.triangulation}. Write under ${shared.data.outputDir}/${shared.data.topicSlug}/ (a flat directory — no numbered subfolders, no manifests): report.md, bibliography.md, and evidence.csv (claim, source, rating, url).
                    No reviewer verdict attached means this is round 1: implement the full delivery. On every later round a verdict IS attached — fix every blocker it names.
                    Return a work summary: what was written, how citations were verified, open concerns.`,
      output: shared.data.workSummary,
      include: [shared.data.verdict1.optional(), shared.data.workSummary.optional()],
    });

    smart.prompt("Building Review", {
      input: input.prompt`
                    Review the research delivered for ${shared.data.topic} against the plan ${shared.data.streamPlan} and triangulation ${shared.data.triangulation}. Work summary: ${shared.data.workSummary}.
                    A BLOCKER is an uncited factual claim, a plan stream left uncovered, an unaddressed cross-stream contradiction, a source below C-rating used as sole support for a critical claim, or a file written outside the flat ${shared.data.outputDir}/${shared.data.topicSlug}/ layout. Everything else is advisory.
                    Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                    Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict1,
      include: [shared.data.verdict1.optional()],
    });

    deriveParkReportStep(shared.data.verdict1, { parkReportSlot: shared.data.parkReport });

    flow.until(condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.stage.commit.shippingCommitTail({
    agent: scribe,
    text: input.prompt`
                The review for the research on ${shared.data.topic} has ended approved and the work is being committed.
                Write a concise commit message from the work summary ${shared.data.workSummary} and the verdict ${shared.data.verdict1}: a short subject line naming the topic, then one paragraph on what was researched and delivered at ${shared.data.reportPath}.
                Return exactly that message. Do not run git commands and do not write files.`,
  });

  return {
    commitReport: shared.data.commitReport,
    parkReportPort: shared.data.parkReportPort,
    researchReportPort: shared.data.researchReportPort,
    reportPathPort: shared.data.reportPathPort,
  };
}
