import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

// The tree lane (owner order 2026-09-04): ctx-annotate opens once, at the
// start, on the run's working tree. The owner walks the codebase and
// annotates the places to change; those annotations are the task. From
// there the lane is the basic one — draft, then implement / review /
// refine, then commit — with no plan gate, no verdict gate and no summons.
export default function () {
  cdk.defineVariant("Annotate", {
    description:
      "Tree-annotated implementation: the owner marks places in ctx-annotate's tree view, the run drafts a plan from the annotations, then implements, reviews, refines and commits.",
    metadata: { tag: [...shared.metadata.tag, "annotate"] },
  });

  shared.step.diff.baseline("Capture the session base");
  shared.step.annotate.treeGate("Annotate the codebase");

  // Closing the tree without annotating ends the run with nothing to do.
  cdk.flow.when(
    "Owner annotated",
    cdk.condition.not(cdk.condition.equals(shared.data.annotations, "none")),
    () => {
      shared.step.draft.composeFromAnnotations("Draft the implementation plan");

      cdk.flow.loop("Reviewed refinement", (loop) => {
        // No owner gate in this lane, so the loop carries its own ceiling;
        // exhaustion continues to the commit with the last verdict unresolved,
        // the same policy as the complex lane.
        loop.maxIterations(5, { onExhausted: cdk.signal.Continue });
        shared.step.work.implement("Implement the task");
        shared.step.diff.capture("Capture the changed files");
        shared.step.review.primaryAnnotated("Review the implementation");
        loop.untilAll([cdk.condition.equals(shared.data.verdict1.status, "approved")]);
      });

      shared.step.git.status("Check working tree status");
      cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
        shared.step.git.commitMessageAnnotated("Write the commit message");
        shared.step.git.commitStage("Stage all changes");
        shared.step.git.commitSubmit("Commit the work");
      });
    },
  );
}
