// refactor-guarded: the quick loop, unchanged, except the final commit
// routes through `ctx-gate run --` and waits for the owner's approval
// before it lands — an approval gate stands between the work and the
// commit, adding the owner as one more approver on top of the reviewer.
import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import * as step from "./step/index.ts";

export default function () {
  cdk.defineVariant("Guarded", {
    description:
      "Refactor one module or entity quickly: checklist, implementation, review, fixing, then a ctx-gate-held commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.checklist.compose("Checklist the target");

  cdk.flow.loop("Reviewed refinement", (loop) => {
    loop.maxIterations(2, { onExhausted: cdk.signal.Continue });
    shared.step.implement.apply("Implement the checklist");
    shared.step.review.judge("Review the implementation");
    loop.until(cdk.condition.equals(shared.data.verdict1.status, "approved"));
  });

  step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(step.git.status.output), () => {
    step.git.commitMessage("Write the commit message");
    step.git.commitStage("Stage all changes");
    step.git.commitSubmit("Commit the refactor (awaiting ctx-gate approval)");
  });

  return { commitReport: shared.data.commitReport };
}
