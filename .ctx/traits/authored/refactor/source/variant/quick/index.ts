import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Quick", {
    description: "Refactor one module or entity quickly: checklist, implementation, review, fixing, commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.stage.checklist.compose("Checklist the target");

  cdk.flow.loop("Reviewed refinement", (loop) => {
    loop.maxIterations(2, { onExhausted: cdk.signal.Continue });

    shared.stage.implement.apply("Implement the checklist");
    shared.stage.review.judge("Review the implementation");

    loop.until(cdk.condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.stage.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.stage.git.status.output), () => {
    shared.stage.git.commitMessage("Write the commit message");
    shared.stage.git.commitStage("Stage all changes");
    shared.stage.git.commitSubmit("Commit the refactor");
  });

  return { commitReport: shared.data.commitReport };
}
