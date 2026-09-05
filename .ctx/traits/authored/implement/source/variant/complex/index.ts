import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

// The doubly-reviewed lane: the basic stage loop without owner gates or
// summons, with a second, independent reviewer grading every claim beside
// the first. Both must approve. No iteration ceiling (0281 rulings: no
// round fuses); the run's total budget is the bound.
const STAGE_SECONDS = 4 * 60 * 60;

export default function () {
  cdk.defineVariant("Complex", {
    description: "Doubly-reviewed implementation: plan, then stage by stage — work until claimed, prove, two reviews, commit.",
    metadata: { tag: [...shared.metadata.tag, "multi-agent"] },
  });

  shared.step.diff.baseline("Capture the session base");
  shared.step.draft.compose("Draft the implementation plan");
  shared.step.review.baseline("Review the tree before any work");

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
      shared.step.review.primary("Review the claim");
      shared.step.review.secondary("Cross-review the claim");

      cdk.flow.when(
        "Stage committed",
        cdk.condition.fieldEquals(shared.data.verdict1, "closed-stage.commit", true),
        () => {
          shared.step.git.carryClosedStage("Carry the closed stage");
          shared.step.git.stageCommitMessage("Write the stage commit message");
          shared.step.git.commitStage("Stage all changes");
          shared.step.git.commitSubmit("Commit the stage");
          shared.step.git.taskBranch("Move the task branch");
        },
      );

      stage.untilAll([
        cdk.condition.equals(shared.data.verdict1.status, "approved"),
        cdk.condition.equals(shared.data.verdict2.status, "approved"),
      ]);
    });
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
    shared.step.git.taskBranch("Move the task branch");
  });
}
