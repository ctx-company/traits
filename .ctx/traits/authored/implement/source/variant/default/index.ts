// implement-default: worker -> reviewer refinement loop until approved, then
// commit. The loop IS the mechanism (2026-07-27 doctrine, kept through the
// 2026-08-17 slim-down): the reviewer says "not yet" and the worker goes
// again, until approval or the budget is gone — exhaustion aborts with its
// stop reason, commits nothing.
import { condition, defineVariant, flow, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Default", {
    name: "Implement",
    description:
      "Reviewed implementation: the worker implements the board task, a reviewer grinds a bounded refinement loop against the task's own Done when until approved — then commit.",
    metadata: { tag: shared.metadata.tag },
  });
  useBehavior(shared.metadata.FAMILY_BEHAVIOR);
  useIntent(shared.intent.REVIEWED_INTENT);

  flow.loop("Building", (loop) => {
    loop.maxIterations(6, { onExhausted: "abort" });

    shared.stage.implement.applyReviewed("Building Produce");
    shared.stage.diff.changedFilesStep();
    shared.stage.review.first("Building Review");

    flow.until(condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.familyCommitTail({
    id: "shipping",
    scribe: { agent: shared.agent.scribe, text: shared.stage.git.scribeText },
    receipt: shared.data.commitOutput,
  });

  return { commitReport: shared.data.commitReport };
}
