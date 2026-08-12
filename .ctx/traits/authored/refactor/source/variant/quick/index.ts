import { condition, defineVariant, flow } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import * as stage from "./stage/index.ts";

export default function () {
  defineVariant("Quick", {
    metadata: { tag: shared.metadata.tag },
    summary: "Refactor one module or entity quickly: checklist, implementation, review, fixing, commit.",
    description: "Refactor one module or entity quickly: checklist, implementation, review, fixing, commit.",
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
    stage.git.commitSubmit("Commit the refactor");
  });

  return { commitReport: shared.data.commitReport };
}
