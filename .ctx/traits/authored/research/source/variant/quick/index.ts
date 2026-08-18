// research-quick: an inline scope note, one bounded worker evidence pass,
// then a single-reviewer grind loop until approved — then commit. Lean, like
// implement-quick: no leftovers adjudication, no dual review. The reviewer
// loop is bounded and parks honestly (typed park report) on exhaustion.
import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, signal, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import { scopeNote } from "./data.ts";

export default function () {
  defineVariant("Quick", {
    name: "Research (Quick)",
    summary:
      "Quick dogfood research procedure: scope the topic, research it in one bounded worker pass, grind a single reviewer loop until approved, then commit.",
    metadata: { tag: shared.metadata.quickTag },
    description:
      "Research one topic quickly: an inline scope note, one bounded worker research pass, a single bounded reviewer loop, then commit brief.md and sources.md.",
  });
  useBehavior(shared.behavior.family);
  useIntent(shared.intent.quick);

  const smart = reviewerRole(
    "smart-1",
    "Scopes the topic and is the sole reviewer of the delivered research.",
    "Scoping and reviewer role.",
  );
  const worker = workerRole("worker", "Researches the topic and applies reviewer fixes.");
  const scribe = scribeRole(
    "scribe",
    "Writes the commit message for the completed research from the scope note and verdict",
  );

  shared.stage.derive.deriveTopicSlugStep();
  shared.stage.derive.deriveReportPathStep("brief.md");

  smart.prompt("Scope the topic", {
    input: input.prompt`
                Write a short scope note for researching ${shared.data.topic}: the core question, two to four sub-questions a worker should cover, and any explicit exclusions.
                Reference ${shared.resource.researchStandards} for source-quality expectations. Do not research anything yet.`,
    output: scopeNote,
  });

  flow.loop("Researching", (loop) => {
    loop.maxIterations(3, { onExhausted: signal.Abort });

    worker.prompt("Researching Produce", {
      input: input.prompt`
                    Research ${shared.data.topic} against the scope note ${scopeNote} and write the deliverable under ${shared.data.outputDir}/${shared.data.topicSlug}/ (a flat directory — no numbered subfolders, no manifests): brief.md (findings with inline citations) and sources.md (a bibliography, canonical A-E source ratings per ${shared.resource.sourceQualityGuide}, formatted per ${shared.resource.citationStyle}).
                    No reviewer verdict attached means this is round 1: cover every sub-question from the scope note. On every later round a verdict IS attached — fix every blocker it names.
                    Apply ${shared.resource.researchStandards} throughout: cite every factual claim, prefer A/B-rated sources, record contradictions and gaps explicitly.
                    Return a work summary: what was written, source counts and ratings, open concerns.`,
      output: shared.data.workSummary,
      include: [shared.data.verdict1.optional(), shared.data.workSummary.optional()],
    });

    smart.prompt("Researching Review", {
      input: input.prompt`
                    Review the research delivered for ${shared.data.topic} against the scope note ${scopeNote}. Work summary: ${shared.data.workSummary}.
                    A BLOCKER is an uncited factual claim, a sub-question left uncovered, a source below C-rating used as sole support for a critical claim, or a file written outside the flat ${shared.data.outputDir}/${shared.data.topicSlug}/ layout. Everything else is advisory.
                    Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                    Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict1,
      include: [shared.data.verdict1.optional()],
    });

    loop.until(condition.equals(shared.data.verdict1.status, "approved"));
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
    researchReportPort: shared.data.researchReportPort,
    reportPathPort: shared.data.reportPathPort,
  };
}
