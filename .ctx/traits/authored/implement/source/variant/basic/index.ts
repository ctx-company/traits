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
  shared.step.draft.compose("Draft the implementation plan");

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
    // no iteration ceiling again. Approval alone does not end it — the
    // owner can overrule an approved verdict by annotating it.
    loop.untilAll([
      cdk.condition.equals(shared.data.verdict1.status, "approved"),
      cdk.condition.equals(shared.data.gateAnswer, "accepted"),
    ]);
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.notify.update("Notify: committed", "Commit the work");
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
  });
  shared.step.notify.finish("Close the owner notification");

  return { commitReport: shared.data.commitReport };
}
