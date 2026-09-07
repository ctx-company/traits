import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

// The unreviewed lane: one plan, one walk, one commit. No reviewer, so no step
// list — the worker implements the plan directly, dispatch after dispatch,
// until it returns done or blocked. One pass, as the name says.
export default function () {
  cdk.defineVariant("Quick", {
    description: "Unreviewed implementation: plan, implement to done, commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.plan.compose("Draft the implementation plan");

  cdk.flow.loop("Implement the plan", (work) => {
    shared.step.work.implementPlan("Implement the plan");
    work.untilAll([cdk.condition.not(cdk.condition.fieldEquals(shared.data.report, "status", "not-done"))]);
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
  });
}
