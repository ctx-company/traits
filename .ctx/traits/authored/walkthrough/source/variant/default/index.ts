// walkthrough-default: one bounded investigation pass producing the typed
// node tree, a single-reviewer grind loop until the tree is grounded and
// well-shaped, then a deterministic render tail. Lean like research-quick:
// no dual review, no commit tail — the artifact lands gitignored under
// .hidden/walkthroughs, so git semantics never enter the procedure.
import { reviewerRole, workerRole } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, intent, signal, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Default", {
    name: "Walkthrough (Default)",
    summary:
      "Investigate a codebase topic into a typed flat node tree (coarse to fine, every node code-anchored), review it until grounded, then render a self-contained interactive treemap HTML.",
    metadata: { tag: shared.metadata.defaultTag },
    description:
      "Explain one codebase topic top-to-bottom: an investigator surveys the repository with its own tools and returns a flat node list (one overview root, at most three levels below, every node carrying verified file:line refs); a reviewer grinds until the tree is honest; a command step renders the interactive treemap walkthrough.",
  });
  useBehavior(shared.behavior.family);
  useIntent({
    require: [intent.Correctness, intent.Leanness, intent.ReviewBeforeFinal],
    avoid: [intent.ScopeCreep, intent.RubberStampReview],
  });

  const investigator = workerRole(
    "investigator",
    "Explores the repository with its own tools and produces the walkthrough node tree.",
  );
  const reviewer = reviewerRole(
    "smart-1",
    "Sole reviewer: verifies every ref against the actual files and the tree's shape and register.",
    "Review role.",
  );

  shared.step.derive.deriveTopicSlugStep();
  shared.step.derive.deriveHtmlPathStep();

  flow.loop("Investigating", (loop) => {
    loop.maxIterations(3, { onExhausted: signal.Abort });

    investigator.prompt("Investigating Produce", {
      input: input.prompt`
                Investigate ${shared.data.topic} in THIS repository with your own tools (search, read files) and return the complete walkthrough node list. Follow ${shared.resource.walkthroughStandards} exactly.
                Shape: exactly one root node with no parent (kind "overview") explaining the topic at architecture level; children partition their parent's territory without overlap; at most three levels below the root; every id kebab-case and unique; every parent an existing id.
                Grounding: every node carries at least one ref whose path and line span you VERIFIED by opening the file — the treemap derives tile sizes from these spans, so spans must honestly cover the code the node explains. A ref you did not open is a fabrication.
                Register: the root narrates intent and architecture; mid nodes narrate responsibilities and collaborations; leaves narrate the actual mechanics precisely. Explanations are plain prose paragraphs; inline backtick code allowed.
                No reviewer verdict attached means this is round 1: survey first, then write the tree. On every later round a verdict IS attached — fix every blocker it names and return the corrected COMPLETE list.
                Also return nothing else: the node list is the deliverable.`,
      output: shared.data.walkthroughNodes,
      include: [shared.data.verdict1.optional(), shared.data.walkthroughNodes.optional()],
    });

    investigator.prompt("Investigating Summarize", {
      input: input.prompt`
                Summarize the investigation you just delivered for ${shared.data.topic}: what you explored, how you verified refs, coverage decisions you made, and open concerns. A few sentences of plain prose.`,
      output: shared.data.workSummary,
      include: [shared.data.walkthroughNodes.optional(), shared.data.workSummary.optional()],
    });

    reviewer.prompt("Investigating Review", {
      input: input.prompt`
                Review the walkthrough node list ${shared.data.walkthroughNodes} delivered for ${shared.data.topic} against ${shared.resource.walkthroughStandards}. Investigator summary: ${shared.data.workSummary}.
                Verify with your OWN tools: open a sample of every node's refs and confirm the path exists and the span covers what the explanation claims; confirm exactly one parentless root, no orphan parents, no cycle, depth at most three below the root; confirm children partition parents and the register matches the layer.
                A BLOCKER is: a ref whose file or span does not support the explanation, a missing/duplicated id, an orphaned or cyclic parent link, more than one root, depth beyond three, a layer whose explanation is at the wrong register, or a major area of the topic with no node.
                Everything else is advisory. Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict1,
      include: [shared.data.verdict1.optional()],
    });

    loop.until(condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.step.render.renderWalkthroughStep();

  return {
    walkthroughPathPort: shared.data.walkthroughPathPort,
    walkthroughSummaryPort: shared.data.walkthroughSummaryPort,
    renderReportPort: shared.data.renderReportPort,
  };
}
