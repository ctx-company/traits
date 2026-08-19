import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Quick", {
    description: "Refactor one module or entity quickly: checklist, implementation, review, fixing, commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.checklist.compose("Checklist the target");

  cdk.flow.loop("Reviewed refinement", (loop) => {
    loop.maxIterations(2, { onExhausted: cdk.signal.Continue });

    shared.step.implement.apply("Implement the checklist");
    shared.step.review.judge("Review the implementation");

    loop.until(cdk.condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the refactor");
  });

  return { commitReport: shared.data.commitReport };
}
