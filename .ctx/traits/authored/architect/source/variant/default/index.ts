import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Default", {
    description: "Ground one draft task, rewrite it in place as a ready execution plan, and reject open-endedness.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.ground.resolve("Resolve the task");
  cdk.step.project("Extract target file", {
    projections: [{ source: shared.data.target, field: "file", destination: shared.data.targetFile }],
  });
  shared.step.snapshot.capture("Snapshot resolved task");
  shared.step.ground.inspect("Ground the task");

  cdk.flow.loop("Architect refinement", (loop) => {
    // 0253.7 (owner-gated): the loop's ceiling is the owner's approval,
    // not an iteration count — 24 is a runaway backstop, and the run
    // budget is the outer bound. Each critic-approved iteration parks on
    // the approval gate; the owner's answer either exits the loop
    // ("approved") or becomes binding corrections for the next rewrite.
    // The snapshot refresh after the gate legalizes plan edits the owner
    // made by hand while the run waited.
    loop.maxIterations(24, { onExhausted: cdk.signal.Abort });
    shared.step.snapshot.assertUnchanged("Assert task unchanged before rewrite");
    shared.step.rewrite.inPlace("Rewrite the task in place");
    shared.step.snapshot.capture("Refresh rewritten task snapshot");
    shared.step.critic.openEndedness("Criticize open-endedness");
    shared.step.snapshot.assertUnchanged("Assert task unchanged after critic");
    cdk.flow.when(
      "Present the plan to the owner",
      cdk.condition.equals(shared.data.criticVerdict.status, "approved"),
      () => {
        shared.step.owner.approvalGate("Await the owner's verdict");
        shared.step.snapshot.capture("Refresh snapshot after owner review");
      },
    );
    loop.untilAll([
      cdk.condition.equals(shared.data.criticVerdict.status, "approved"),
      cdk.condition.equals(shared.data.ownerAnswer, "approved"),
    ]);
  });

  shared.step.commit.commitStep("Commit the rewritten task");

  return { receipts: shared.data.receipts };
}
