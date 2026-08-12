import { condition, defineVariant, flow } from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";
import * as stage from "./stage/index.ts";

export default function () {
  defineVariant("Guided", {
    description:
      "Collect human annotations interactively, plan them as a checklist, implement every item in a reviewed loop, and commit.",
    summary: "Turns human annotations into implemented, reviewed changes.",
    metadata: { tag: shared.metadata.tag },
  });

  stage.annotate.collect("Collect annotations (ctx-annotate)");
  stage.checklist.compose("Build the checklist");

  flow.loop("Reviewed refinement", (loop) => {
    loop.maxIterations(2, { onExhausted: "continue" });
    shared.stage.implement.apply("Implement the checklist");
    shared.stage.review.judge("Review the implementation");
    loop.until(condition.fieldEquals(shared.stage.review.judge.output, "status", "approved"));
  });

  shared.stage.git.status("Check working tree status");
  flow.when("Maybe Commit", condition.not(condition.empty(shared.stage.git.status.output)), () => {
    stage.git.commitMessage("Write the commit message");
    shared.stage.git.commitStage("Stage all changes");
    shared.stage.git.commitSubmit("Commit the refactor");
  });

  return { commitReport: shared.data.commitReport };
}
