import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";
import * as shared from "#trait/shared/index.ts";

// The stage loop (0281.7). The contract is fixed before work: the plan's
// stages carry their goal, proof and commit flag, the owner accepts them at
// the plan gate, and a baseline review opens the ledger — every step with
// a frozen done-when — and writes the first brief. Then, per stage: the
// worker is dispatched again and again on the brief until it claims the
// stage complete and the stage's proof passes, or it reports itself
// blocked, or the stage's time is spent; only then does the reviewer run,
// grading the claim against the frozen text, appending discoveries, and
// moving the brief. A stage the plan marks `commit` is committed the
// moment it flips to done, and the task branch follows it.
//
// How long one stage may be worked before a review is forced, measured on
// the loop's own clock (active-drive seconds since its first dispatch).
// Each dispatch is bounded by the seat's frame budget on top of this.
const STAGE_SECONDS = 4 * 60 * 60;

export default function () {
  cdk.defineVariant("Basic", {
    description: "Reviewed implementation: plan, then stage by stage — work until claimed, prove, review, commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.notify.begin("Open the owner notification");

  shared.step.notify.update("Notify: session base", "Capture the session base");
  shared.step.diff.baseline("Capture the session base");

  shared.step.notify.update("Notify: plan drafted", "Draft the implementation plan");
  // 0281.3: the owner sees the plan before any implement round. The plan
  // is redrafted with the owner's corrections until accepted; with
  // plan-gate=off the gate accepts on the first pass.
  cdk.flow.loop("Plan approval", (loop) => {
    shared.step.draft.compose("Draft the implementation plan");
    shared.step.annotate.digestPlan("Digest the plan for annotation");
    shared.step.annotate.carryPlanSurface("Carry the plan surface");
    shared.step.annotate.planApprovalGate("Annotate the plan");
    shared.step.annotate.recordPlanRuling("Record the owner's plan correction");
    loop.untilAll([cdk.condition.equals(shared.data.planAnswer, "accepted")]);
  });

  // The baseline: the ledger and the first brief exist before the first
  // dispatch, so every claim is graded against text that preceded it.
  shared.step.notify.update("Notify: baseline review", "Review the tree before any work");
  shared.step.review.baseline("Review the tree before any work");

  // Read once, on the baseline verdict: a tree that already meets the plan
  // skips the loop. Inside the loop the body is unconditional — a guard on
  // the loop body that read the previous iteration's verdict would see a
  // stale value (a loop-body slot from an earlier iteration never satisfies
  // this loop's guards) and skip every iteration after the first, spinning
  // the loop to its control budget. The `until` at the end of an iteration
  // reads the verdict that iteration's review wrote, so it stays fresh.
  cdk.flow.when("Work remains", cdk.condition.equals(shared.data.verdict1.status, "revise"), () => {
    cdk.flow.loop("Stage loop", (stage) => {
      shared.step.carry.carryBrief("Carry the brief");
      shared.step.notify.update("Notify: working the stage", "Working the stage");

      // The work loop: dispatches chain on the worker's own report until
      // the claim holds and the proof passed, the worker is blocked, or the
      // stage's time is spent. A dispatch that does not claim runs no
      // proof and reaches no reviewer.
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
      shared.step.review.primarySummoning("Review the claim");
      shared.step.notify.reviewUpdate("Notify: review verdict");
      shared.step.notify.update("Notify: awaiting annotations", "awaiting owner annotations");
      shared.step.annotate.carrySurface("Carry the annotation surface");
      shared.step.annotate.verdictGate("Annotate the verdict");
      shared.step.annotate.recordRuling("Record the owner ruling");
      shared.step.notify.gateResult("Notify: owner ruling");

      // 0281.1: an annotated verdict is applied by the reviewer before the
      // next work loop — the worker only ever sees the applied verdict.
      cdk.flow.when(
        "Owner annotated",
        cdk.condition.not(cdk.condition.equals(shared.data.gateAnswer, "accepted")),
        () => {
          shared.step.review.apply("Apply the owner's annotations");
        },
      );

      cdk.flow.when("Owner ruling", cdk.condition.signal(agents.needsOwnerSignal), () => {
        // owner-ruling=off (unattended nights): the summons branch is skipped —
        // the escalation stays in the verdict for morning review, nothing parks.
        cdk.flow.when(
          "Owner reachable",
          cdk.condition.not(cdk.condition.equals(shared.data.ownerRuling, "off")),
          () => {
            const ruling = shared.step.summon.ask("Summon the owner", agents.needsOwnerSignal);
            shared.step.summon.record("Record the owner's ruling", agents.needsOwnerSignal, ruling.result);
          },
        );
      });

      // A stage the plan marks `commit` lands the moment it flips to done.
      cdk.flow.when(
        "Stage committed",
        cdk.condition.fieldEquals(shared.data.verdict1, "closed-stage.commit", true),
        () => {
          shared.step.git.carryClosedStage("Carry the closed stage");
          shared.step.notify.update("Notify: stage committed", "Commit the stage");
          shared.step.git.stageCommitMessage("Write the stage commit message");
          shared.step.git.commitStage("Stage all changes");
          shared.step.git.commitSubmit("Commit the stage");
          shared.step.git.taskBranch("Move the task branch");
        },
      );

      // The exit reads the APPLIED verdict (0281.1): an approved verdict the
      // owner annotated with new work comes back as revise and the loop
      // continues; a revise verdict the owner overruled to approved ends it.
      stage.untilAll([cdk.condition.equals(shared.data.verdict1.status, "approved")]);
    });
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.notify.update("Notify: committed", "Commit the work");
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
    shared.step.git.taskBranch("Move the task branch");
  });
  shared.step.notify.finish("Close the owner notification");
}
