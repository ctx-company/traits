import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import * as step from "./step/index.ts";

export default function () {
  cdk.defineVariant("Guided", {
    description:
      "Collect human annotations interactively, plan them as a checklist, implement every item in a reviewed loop, and commit.",
    metadata: { tag: shared.metadata.tag },
  });

  step.annotate.collect("Collect annotations (ctx-annotate)");
  step.checklist.compose("Build the checklist");

  cdk.flow.loop("Reviewed refinement", (loop) => {
    loop.maxIterations(2, { onExhausted: cdk.signal.Continue });
    shared.step.implement.apply("Implement the checklist");
    shared.step.review.judge("Review the implementation");
    loop.until(cdk.condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the refactor");
  });

  return { commitReport: shared.data.commitReport };
}
