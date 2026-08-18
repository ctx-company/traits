import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Complex", {
    description: "Refactor procedure: survey, frame, design, implement, refine, and commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.stage.survey.gather("Survey the target");
  shared.stage.frame.select("Frame the problem");
  shared.stage.design.draft("Design the boundary");

  cdk.flow.loop("Doubly-reviewed verbatim refinement", (loop) => {
    loop.maxIterations(6, { onExhausted: "continue" });

    shared.stage.implement.apply("Implement the design specification");
    shared.stage.review.primary("Review refactor");
    shared.stage.review.secondary("Cross-review refactor");

    cdk.flow.untilAll([
      cdk.condition.equals(shared.data.verdict1.status, "approved"),
      cdk.condition.equals(shared.data.verdict2.status, "approved"),
    ]);
  });

  shared.stage.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.stage.git.status.output), () => {
    shared.stage.git.commitMessage("Write the commit message");
    shared.stage.git.commitStage("Stage all changes");
    shared.stage.git.commitSubmit("Commit the refactor");
  });

  return { commitReport: shared.data.commitReport };
}
