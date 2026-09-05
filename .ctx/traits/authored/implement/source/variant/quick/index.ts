import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

// The unreviewed lane: one plan, one dispatch, one commit. No verdict, so
// no brief — the worker works the plan stage by stage on its own.
export default function () {
  cdk.defineVariant("Quick", {
    description: "Unreviewed implementation: plan, implement, commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.draft.compose("Draft the implementation plan");
  shared.step.work.implementPlan("Implement the plan");

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
  });
}
