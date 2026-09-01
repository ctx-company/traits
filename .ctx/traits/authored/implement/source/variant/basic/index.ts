import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";
import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Basic", {
    description: "Reviewed implementation: implement, review, refine & commit.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.notify.begin("Open the owner notification");
  shared.step.diff.baseline("Capture the session base");
  shared.step.notify.update("Notify: session base", "Capture the session base");
  shared.step.draft.compose("Draft the implementation plan");
  shared.step.notify.update("Notify: plan drafted", "Draft the implementation plan");

  cdk.flow.loop("Reviewed refinement", (loop) => {
    shared.step.work.implement("Implement the task");
    shared.step.notify.update("Notify: implement pass", "Implement the task");
    shared.step.diff.capture("Capture the changed files");
    shared.step.review.primarySummoning("Review the implementation");
    shared.step.notify.reviewUpdate("Notify: review verdict");

    cdk.flow.when("Owner ruling", cdk.condition.signal(agents.needsOwnerSignal), () => {
      const ruling = shared.step.summon.ask("Summon the owner", agents.needsOwnerSignal);
      shared.step.summon.record("Record the owner's ruling", agents.needsOwnerSignal, ruling.result);
    });

    // Owner ruling 2026-09-01: three review rounds, then stop refining.
    // Exhaustion continues past the loop — the work proceeds to commit with
    // the last verdict unresolved, and the task's own checks plus the merge
    // gate remain the landing authority. Abort here would strand the work.
    loop.maxIterations(3, { onExhausted: cdk.signal.Continue });
    loop.until(cdk.condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
    shared.step.notify.update("Notify: committed", "Commit the work");
  });
  shared.step.notify.finish("Close the owner notification");

  return { commitReport: shared.data.commitReport };
}
