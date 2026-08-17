// implement-complex: worker -> two INDEPENDENT reviewers, both must approve
// each round before the loop exits — then commit. Same shape as default
// with a second, independent verdict; exhaustion aborts with its stop
// reason, commits nothing.
import { condition, defineVariant, flow, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Complex", {
    name: "Implement (Complex)",
    description:
      "Doubly-reviewed implementation: the worker implements the board task and two independent reviewers must both approve against the task's own Done when — then commit.",
    metadata: { tag: [...shared.metadata.tag, "multi-agent"] },
  });
  useBehavior(shared.metadata.FAMILY_BEHAVIOR);
  useIntent(shared.intent.REVIEWED_INTENT);

  flow.loop("Building", (loop) => {
    loop.maxIterations(8, { onExhausted: "abort" });

    shared.stage.implement.applyDualReviewed("Building Produce");
    shared.stage.diff.changedFilesStep();
    shared.stage.review.first("Building Review 1");
    shared.stage.review.second("Building Review 2");

    flow.untilAll([
      condition.equals(shared.data.verdict1.status, "approved"),
      condition.equals(shared.data.verdict2.status, "approved"),
    ]);
  });

  shared.familyCommitTail({
    id: "shipping",
    scribe: { agent: shared.agent.scribe, text: shared.stage.git.scribeText },
    receipt: shared.data.commitOutput,
  });

  return { commitReport: shared.data.commitReport };
}
