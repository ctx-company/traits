import * as cdk from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Complex", {
    description: "Reviewed implementation: implement, review, refine & commit.",
    metadata: { tag: [...shared.metadata.tag, "multi-agent"] },
  });

  shared.step.diff.baseline("Capture the session base");
  shared.step.draft.compose("Draft the implementation plan");

  cdk.flow.loop("Doubly-reviewed refinement", (loop) => {
    // Owner ruling 2026-09-01: three review rounds, then stop refining.
    // Exhaustion continues past the loop — the work proceeds to commit with
    // the last verdicts unresolved, and the task's own checks plus the merge
    // gate remain the landing authority. Abort here would strand the work.
    loop.maxIterations(3, { onExhausted: cdk.signal.Continue });
    shared.step.work.implement("Implement the task");
    shared.step.diff.capture("Capture the changed files");
    shared.step.review.primary("Review the implementation");
    shared.step.review.secondary("Cross-review the implementation");

    loop.untilAll([
      cdk.condition.equals(shared.data.verdict1.status, "approved"),
      cdk.condition.equals(shared.data.verdict2.status, "approved"),
    ]);
  });

  shared.step.git.status("Check working tree status");
  cdk.flow.when("Maybe Commit", cdk.condition.notEmpty(shared.step.git.status.output), () => {
    shared.step.git.commitMessage("Write the commit message");
    shared.step.git.commitStage("Stage all changes");
    shared.step.git.commitSubmit("Commit the work");
  });

  return { commitReport: shared.data.commitReport };
}
