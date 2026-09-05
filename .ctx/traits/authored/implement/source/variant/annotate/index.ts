import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

// The tree lane (owner order 2026-09-04): ctx-annotate opens once, at the
// start, on the run's working tree. The owner walks the codebase and
// annotates the places to change; those annotations are the task. From
// there the lane is the basic stage loop — draft, baseline, then per stage
// work until claimed, prove, review, commit — with no plan gate, no
// verdict gate and no summons. No task port, so no task branch.
const STAGE_SECONDS = 4 * 60 * 60;

export default function () {
  cdk.defineVariant("Annotate", {
    description:
      "Tree-annotated implementation: the owner marks places in ctx-annotate's tree view, the run drafts a plan from the annotations, then works it stage by stage — claim, prove, review, commit.",
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
      shared.step.review.baselineAnnotated("Review the tree before any work");

      // Read once on the baseline verdict; the loop body itself is
      // unconditional (see the basic variant for why a body guard on the
      // previous iteration's verdict cannot work).
      cdk.flow.when("Work remains", cdk.condition.equals(shared.data.verdict1.status, "revise"), () => {
        cdk.flow.loop("Stage loop", (stage) => {
          shared.step.carry.carryBrief("Carry the brief");

          cdk.flow.loop("Work the stage", (work) => {
            shared.step.work.implement("Implement the stage");
            cdk.flow.when("Claimed", cdk.condition.fieldEquals(shared.data.report, "claim", "complete"), () => {
              shared.step.proof.run("Prove the claim");
            });
            work.untilAll([
              cdk.condition.any([
                cdk.condition.all([
                  cdk.condition.fieldEquals(shared.data.report, "claim", "complete"),
                  cdk.condition.equals(shared.data.proofResult, "pass"),
                ]),
                cdk.condition.not(cdk.condition.fieldEquals(shared.data.report, "blocked", "")),
                cdk.condition.loopElapsedAtLeast(STAGE_SECONDS),
              ]),
            ]);
          });

          shared.step.diff.capture("Capture the changed files");
          shared.step.review.primaryAnnotated("Review the claim");

          cdk.flow.when(
            "Stage committed",
            cdk.condition.fieldEquals(shared.data.verdict1, "closed-stage.commit", true),
            () => {
              shared.step.git.carryClosedStage("Carry the closed stage");
              shared.step.git.stageCommitMessage("Write the stage commit message");
              shared.step.git.commitStage("Stage all changes");
              shared.step.git.commitSubmit("Commit the stage");
            },
          );
          stage.untilAll([cdk.condition.equals(shared.data.verdict1.status, "approved")]);
        });
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
