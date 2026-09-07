import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

// The doubly-reviewed lane: the basic step-walk without owner gates or
// summons, with a second, independent reviewer re-validating the tree after
// the first and having the final say on the step list. No iteration ceiling
// (0281 rulings: no round fuses); the run's total budget is the bound.
export default function () {
  cdk.defineVariant("Complex", {
    description: "Doubly-reviewed implementation: plan, then walk the steps — work each until done or blocked, two independent reviews, repeat until the list is empty.",
    metadata: { tag: [...shared.metadata.tag, "multi-agent"] },
  });

  shared.step.diff.baseline("Capture the session base");
  shared.step.plan.compose("Draft the implementation plan");
  shared.step.review.baseline("Open the step list");

  cdk.flow.when("Work remains", cdk.condition.notEmpty(shared.data.steps), () => {
    cdk.flow.loop("Review loop", (round) => {
      shared.data.steps.forEach("Work the steps", (step, walk) => {
        cdk.flow.loop("Work the step", (work) => {
          shared.step.work.implement("Implement the step", step);
          work.untilAll([cdk.condition.not(cdk.condition.fieldEquals(shared.data.report, "status", "not-done"))]);
        });
      });

      // Two independent validations of the same tree; the second has the
      // final say on the list for the next pass. Unattended — no summons.
      shared.step.review.validateSolo("Review the stage");
      shared.step.review.crossValidate("Cross-review the stage");

      shared.step.git.status("Check working tree status");
      cdk.flow.when("Commit progress", cdk.condition.notEmpty(shared.step.git.status.output), () => {
        shared.step.git.commitMessage("Write the commit message");
        shared.step.git.commitStage("Stage all changes");
        shared.step.git.commitSubmit("Commit the progress");
        shared.step.git.taskBranch("Move the task branch");
      });

      round.untilAll([cdk.condition.not(cdk.condition.notEmpty(shared.data.steps))]);
    });
  });
}
