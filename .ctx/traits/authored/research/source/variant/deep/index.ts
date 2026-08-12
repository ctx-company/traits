// research-deep: plan 4-6 streams with a mandatory counterevidence stream
// (deterministic gate), research each serially, triangulate, then a
// Chain-of-Verification produce round reviewed by two INDEPENDENT
// reviewers each round (dual-approval, mirroring implement:default) — then
// commit. Both verdicts must approve before the commit tail is reachable.
//
// Deviation from the draft (owner-triage item, 0153): streams are researched
// serially within one worker turn rather than a runtime for-each — see the
// identical note in variant/default/index.ts for why (the functional for-each
// sugar cannot carry a typed item; `modules/core/src/trait/procedure/
// validate.rs:2133`). The plan's own cardinality + counterevidence gate is
// what bounds it.
import { deriveParkReportStep, reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Deep", {
    name: "Research (Deep)",
    summary:
      "Deep dogfood research procedure: plan 4-6 streams including a mandatory counterevidence stream, research each serially, triangulate, apply Chain-of-Verification, then a dual-reviewer refinement loop — then commit.",
    metadata: { tag: shared.metadata.fullTag },
    description:
      "Research one topic exhaustively: plan 4-6 streams including a mandatory counterevidence stream, research each serially, triangulate, apply Chain-of-Verification, then a bounded dual-reviewer refinement loop, then commit report.md, bibliography.md, evidence.csv, and verification.md.",
  });
  useBehavior(shared.behavior.family);
  useIntent(shared.intent.full);

  const smart1 = reviewerRole(
    "smart-1",
    "Plans the research streams, and independently reviews the delivered research each refinement pass.",
    "Planning and first-reviewer role.",
  );
  const smart2 = reviewerRole(
    "smart-2",
    "Independently reviews the delivered research each refinement pass, separately from smart-1.",
    "Second-reviewer role.",
  );
  const worker = workerRole(
    "worker",
    "Researches each stream, triangulates, drafts and verifies the report, and applies reviewer fixes.",
  );
  const scribe = scribeRole("scribe", "Writes the commit message for the completed research from the plan and verdicts");

  const streamCountValid = condition.all([
    condition.any([
      condition.count(shared.data.streamPlan).equals(4),
      condition.count(shared.data.streamPlan).equals(5),
      condition.count(shared.data.streamPlan).equals(6),
    ]),
    condition.count(shared.data.streamPlan).where("kind", "counterevidence").atLeast(1),
  ]);

  shared.stage.derive.deriveTopicSlugStep();
  shared.stage.derive.deriveReportPathStep("report.md");

  flow.loop("Planning", (loop) => {
    loop.maxIterations(3, { onExhausted: "abort" });

    smart1.prompt("Plan the streams", {
      input: input.prompt`
                Plan four to six non-overlapping research streams for ${shared.data.topic}. Each stream gets a stable id, a complete non-overlapping focus question, and a kind: "primary" or "counterevidence".
                At least one stream MUST be kind "counterevidence": its focus is deliberately searching for disconfirming evidence and competing conclusions against the other streams' emerging findings, not merely a skeptical framing of a primary question.
                This is the run's one and only executable stream list, consumed directly by serial per-stream research.`,
      output: shared.data.streamPlan,
      include: [shared.data.streamPlan.optional()],
    });

    flow.until(streamCountValid);
  });

  worker.prompt("Research the streams", {
    input: input.prompt`
                Research every stream in the plan ${shared.data.streamPlan} for the topic ${shared.data.topic}, one stream at a time and in the order planned — never merge two streams' evidence together. For the "counterevidence" stream, actively search for disconfirming evidence and competing conclusions rather than corroborating the other streams.
                Apply ${shared.resource.researchStandards}: cite every factual claim, rate sources per the canonical A-E scale (${shared.resource.sourceQualityGuide}), format citations per ${shared.resource.citationStyle}, and note any counterevidence encountered.
                Return exactly one stream-finding per planned stream (same order, same ids): the stream's id and its cited findings summary. Write nothing to disk yet — this is evidence gathering only.`,
    output: shared.data.findings,
  });

  worker.prompt("Triangulate the findings", {
    input: input.prompt`
                Triangulate the researched findings ${shared.data.findings} for ${shared.data.topic}: where independent streams agree, where they conflict — including what the counterevidence stream surfaced — source-quality ratings, and remaining gaps.
                Return the triangulation.`,
    output: shared.data.triangulation,
  });

  flow.loop("Building", (loop) => {
    loop.maxIterations(8, { onExhausted: "abort" });

    worker.prompt("Building Produce", {
      input: input.prompt`
                    Produce the research delivery for ${shared.data.topic} from the plan ${shared.data.streamPlan}, findings ${shared.data.findings}, and triangulation ${shared.data.triangulation}. Write under ${shared.data.outputDir}/${shared.data.topicSlug}/ (a flat directory — no numbered subfolders, no manifests): report.md, bibliography.md, evidence.csv (claim, source, rating, url), and verification.md.
                    Apply the Chain-of-Verification method from ${shared.resource.researchStandards} to every critical claim before writing verification.md: generate the claim, write a verification question for it, independently answer that question against the cited sources, and revise the claim (or drop it) based on that answer. verification.md records each critical claim, its verification question, the independent answer, and the disposition.
                    No reviewer verdict attached means this is round 1: implement the full delivery. On every later round two independent verdicts ARE attached — fix every blocker either names.
                    Return a work summary: what was written, how citations and critical claims were verified, open concerns.`,
      output: shared.data.workSummary,
      include: [shared.data.verdict1.optional(), shared.data.verdict2.optional(), shared.data.workSummary.optional()],
    });

    smart1.prompt("Building Review 1", {
      input: input.prompt`
                    Review the research delivered for ${shared.data.topic} against the plan ${shared.data.streamPlan} and triangulation ${shared.data.triangulation}. Work summary: ${shared.data.workSummary}.
                    A BLOCKER is an uncited factual claim, a plan stream left uncovered, an unaddressed cross-stream contradiction, a critical claim without a recorded Chain-of-Verification pass in verification.md, a source below C-rating used as sole support for a critical claim, or a file written outside the flat ${shared.data.outputDir}/${shared.data.topicSlug}/ layout. Everything else is advisory.
                    Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                    Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict1,
      include: [shared.data.verdict1.optional()],
    });
    smart2.prompt("Building Review 2", {
      input: input.prompt`
                    Independently review the research delivered for ${shared.data.topic} against the plan ${shared.data.streamPlan} and triangulation ${shared.data.triangulation}. Work summary: ${shared.data.workSummary}. Do not trust the work summary or smart-1's verdict — inspect the written deliverables and cited sources yourself.
                    A BLOCKER is an uncited factual claim, a plan stream left uncovered, an unaddressed cross-stream contradiction, a critical claim without a recorded Chain-of-Verification pass in verification.md, a source below C-rating used as sole support for a critical claim, or a file written outside the flat ${shared.data.outputDir}/${shared.data.topicSlug}/ layout. Everything else is advisory.
                    Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                    Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict2,
      include: [shared.data.verdict2.optional()],
    });

    deriveParkReportStep([shared.data.verdict1, shared.data.verdict2], { parkReportSlot: shared.data.parkReport });

    flow.until(
      condition.all([
        condition.equals(shared.data.verdict1.status, "approved"),
        condition.equals(shared.data.verdict2.status, "approved"),
      ]),
    );
  });

  shared.stage.commit.shippingCommitTail({
    agent: scribe,
    text: input.prompt`
                The dual review for the research on ${shared.data.topic} has ended in agreement and the work is being committed. The final verdicts are ${shared.data.verdict1} and ${shared.data.verdict2} (P414: this step is unreachable while either status is still revise; that case parks instead, with a typed park report, and never reaches here).
                Write a concise commit message from the work summary ${shared.data.workSummary} and those verdicts: a short subject line naming the topic, then one paragraph on what was researched, verified, and delivered at ${shared.data.reportPath}.
                Return exactly that message. Do not run git commands and do not write files.`,
  });

  return {
    commitReport: shared.data.commitReport,
    parkReportPort: shared.data.parkReportPort,
    researchReportPort: shared.data.researchReportPort,
    reportPathPort: shared.data.reportPathPort,
  };
}
