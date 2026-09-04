import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";
import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Basic", {
    description: "Reviewed implementation: implement, review, refine & commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.notify.begin("Open the owner notification");
  cdk.step.command("Carry the owner-ruling mode", {
    id: "carry-owner-ruling",
    argv: ["printf", "%s", shared.data.ownerRuling],
    output: shared.data.ownerRulingMode,
  });

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

  cdk.flow.loop("Reviewed refinement", (loop) => {
    shared.step.notify.update("Notify: implement pass", "Implement the task");
    shared.step.work.implement("Implement the task");

    shared.step.diff.capture("Capture the changed files");
    shared.step.review.primarySummoning("Review the implementation");
    shared.step.notify.reviewUpdate("Notify: review verdict");
    shared.step.notify.update("Notify: awaiting annotations", "awaiting owner annotations");
    shared.step.annotate.carrySurface("Carry the annotation surface");
    shared.step.annotate.verdictGate("Annotate the verdict");
    shared.step.annotate.recordRuling("Record the owner ruling");
    shared.step.notify.gateResult("Notify: owner ruling");

    // 0281.1: an annotated verdict is applied by the reviewer before the
    // next implement round — the worker only ever sees the applied verdict.
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
        cdk.condition.not(cdk.condition.equals(shared.data.ownerRulingMode, "off")),
        () => {
          const ruling = shared.step.summon.ask("Summon the owner", agents.needsOwnerSignal);
          shared.step.summon.record("Record the owner's ruling", agents.needsOwnerSignal, ruling.result);
        },
      );
    });

    // Owner ruling 2026-09-01 (superseding the three-round cap): the
    // annotate gate is the owner's own per-round control, so the loop has
    // no iteration ceiling again. The exit reads the APPLIED verdict
    // (0281.1): an approved verdict the owner annotated with new work
    // comes back as revise and the loop continues; a revise verdict the
    // owner overruled to approved ends it.
    loop.untilAll([cdk.condition.equals(shared.data.verdict1.status, "approved")]);
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.notify.update("Notify: committed", "Commit the work");
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
  });
  shared.step.notify.finish("Close the owner notification");

}
