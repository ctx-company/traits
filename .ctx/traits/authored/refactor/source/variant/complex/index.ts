import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Complex", {
    description: "Refactor procedure: survey, frame, design, implement, refine, and commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.survey.gather("Survey the target");
  shared.step.frame.select("Frame the problem");
  shared.step.design.draft("Design the boundary");

  cdk.flow.loop("Doubly-reviewed verbatim refinement", (loop) => {
    loop.maxIterations(6, { onExhausted: cdk.signal.Continue });

    shared.step.implement.apply("Implement the design specification");
    shared.step.review.primary("Review refactor");
    shared.step.review.secondary("Cross-review refactor");

    loop.untilAll([
      cdk.condition.equals(shared.data.verdict1.status, "approved"),
      cdk.condition.equals(shared.data.verdict2.status, "approved"),
    ]);
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the refactor");
  });

  return { commitReport: shared.data.commitReport };
}
