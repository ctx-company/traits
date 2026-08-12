// refactor-guarded: the quick loop, unchanged, except the final commit
// routes through `ctx-gate run --` and waits for the owner's approval
// before it lands — an approval gate stands between the work and the
// commit, adding the owner as one more approver on top of the reviewer.
import { condition, defineVariant, flow } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

import * as stage from "./stage/index.ts";

export default function () {
  defineVariant("Guarded", {
    description:
      "Refactor one module or entity quickly with the final commit held for the owner's ctx-gate approval: an actionable checklist, implementation, exactly one reviewer pass, one round of fixes if needed, then the gated commit.",
    summary:
      "Quick refactoring procedure with a guarded commit: checklist, implement, one reviewer pass, one round of fixes if needed — then the commit waits for the owner's ctx-gate approval.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.stage.checklist.compose("Checklist the target");

  flow.loop("Reviewed refinement", (loop) => {
    loop.maxIterations(2, { onExhausted: "continue" });
    shared.stage.implement.apply("Implement the checklist");
    shared.stage.review.judge("Review the implementation");
    loop.until(condition.fieldEquals(shared.stage.review.judge.output, "status", "approved"));
  });

  stage.git.status("Check working tree status");
  flow.when("Maybe Commit", condition.not(condition.empty(stage.git.status.output)), () => {
    stage.git.commitMessage("Write the commit message");
    stage.git.commitStage("Stage all changes");
    stage.git.commitSubmit("Commit the refactor (awaiting ctx-gate approval)");
  });

  return { commitReport: shared.data.commitReport };
}
