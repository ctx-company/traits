import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Complex", {
    description: "Reviewed implementation: implement, review, refine & commit.",
    metadata: { tag: [...shared.metadata.tag, "multi-agent"] },
  });

  shared.stage.diff.baseline("Capture the session base");
  shared.stage.draft.compose("Draft the implementation plan");

  cdk.flow.loop("Doubly-reviewed refinement", (loop) => {
    loop.maxIterations(10, { onExhausted: cdk.signal.Abort });

    shared.stage.work.implement("Implement the task");
    shared.stage.diff.capture("Capture the changed files");
    shared.stage.review.primary("Review the implementation");
    shared.stage.review.secondary("Cross-review the implementation");

    loop.untilAll([
      cdk.condition.equals(shared.data.verdict1.status, "approved"),
      cdk.condition.equals(shared.data.verdict2.status, "approved"),
    ]);
  });

  shared.stage.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.stage.git.status.output), () => {
    shared.stage.git.commitMessage("Write the commit message");
    shared.stage.git.commitStage("Stage all changes");
    shared.stage.git.commitSubmit("Commit the work");
  });

  return { commitReport: shared.data.commitReport };
}
