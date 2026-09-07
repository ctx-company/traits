import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

// The tree lane: ctx-annotate opens once, at the start, on the run's working
// tree; the owner's annotations are the task. From there it is the step-walk —
// a plan drafted from the annotations, an open step list, walked and reviewed —
// with no plan gate, no verdict gate, no summons, and no task branch (there is
// no task port). The reviewer decides everything itself (unattended).
export default function () {
  cdk.defineVariant("Annotate", {
    description:
      "Tree-annotated implementation: the owner marks places in ctx-annotate's tree view, the run drafts a plan from the annotations, then walks the steps — work each until done or blocked, review, repeat.",
    metadata: { tag: [...shared.metadata.tag, "annotate"] },
  });

  shared.step.diff.baseline("Capture the session base");
  shared.step.annotate.treeGate("Annotate the codebase");

  // Closing the tree without annotating ends the run with nothing to do.
  cdk.flow.when(
    "Owner annotated",
    cdk.condition.not(cdk.condition.equals(shared.data.annotations, "none")),
    () => {
      shared.step.plan.composeFromAnnotations("Draft the implementation plan");
      shared.step.review.baseline("Open the step list");

      cdk.flow.when("Work remains", cdk.condition.notEmpty(shared.data.steps), () => {
        cdk.flow.loop("Review loop", (round) => {
          shared.data.steps.forEach("Work the steps", (step, walk) => {
            cdk.flow.loop("Work the step", (work) => {
              shared.step.work.implement("Implement the step", step);
              work.untilAll([cdk.condition.not(cdk.condition.fieldEquals(shared.data.report, "status", "not-done"))]);
            });
          });

          shared.step.review.validateSolo("Review the stage");

          shared.step.git.status("Check working tree status");
          cdk.flow.when("Commit progress", cdk.condition.notEmpty(shared.step.git.status.output), () => {
            shared.step.git.commitMessageAnnotated("Write the commit message");
            shared.step.git.commitStage("Stage all changes");
            shared.step.git.commitSubmit("Commit the progress");
          });

          round.untilAll([cdk.condition.not(cdk.condition.notEmpty(shared.data.steps))]);
        });
      });
    },
  );
}
