import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Basic", {
    description: "Reviewed implementation: implement, review, refine & commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.diff.baseline("Capture the session base");
  shared.step.draft.compose("Draft the implementation plan");

  cdk.flow.loop("Reviewed refinement", (loop) => {
    shared.step.work.implement("Implement the task");
    shared.step.diff.capture("Capture the changed files");
    shared.step.review.primary("Review the implementation");

    loop.maxIterations(10, { onExhausted: cdk.signal.Abort });
    loop.until(cdk.condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
  });

  return { commitReport: shared.data.commitReport };
}
