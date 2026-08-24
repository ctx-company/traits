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
    loop.maxIterations(4, { onExhausted: cdk.signal.Abort });
    shared.step.snapshot.assertUnchanged("Assert task unchanged before rewrite");
    shared.step.rewrite.inPlace("Rewrite the task in place");
    shared.step.snapshot.capture("Refresh rewritten task snapshot");
    shared.step.critic.openEndedness("Criticize open-endedness");
    shared.step.snapshot.assertUnchanged("Assert task unchanged after critic");
    loop.until(cdk.condition.equals(shared.data.criticVerdict.status, "approved"));
  });

  return { receipts: shared.data.receipts };
}
