import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Default", {
    description:
      "Investigate an existing functionality, compose a non-code walkthrough of its flows and states (markdown + HTML diagram), and refine it through ctx-annotate until the owner accepts — annotations demand corrections or deeper dives.",
    metadata: { tag: shared.metadata.tag },
  });

  const smart = shared.agent.smart(
    "Investigates the codebase for ground truth, composes the non-code walkthrough and its diagram, and applies the owner's annotations.",
    "Investigation + composition + revision role.",
  );

  shared.step.slugify.slugStep("Derive the artifact slug");
  shared.step.compose.composeStep(smart, "Investigate and compose the walkthrough");
  // Owner acceptance: effectively endless — the ceiling is the owner's
  // empty-annotations acceptance; 500 is a runaway backstop only. The
  // gate opens the diagram and pipes the markdown to ctx-annotate; any
  // annotations become binding corrections or deepening requests applied
  // to BOTH artifacts before the next gate iteration.
  cdk.flow.loop("Owner acceptance", (loop) => {
    loop.maxIterations(500, { onExhausted: cdk.signal.Abort });
    shared.step.gate.acceptanceGate("Present the walkthrough to the owner");
    cdk.flow.when(
      "Apply owner annotations",
      cdk.condition.not(cdk.condition.equals(shared.data.ownerAnswer, "approved")),
      () => {
        shared.step.compose.reviseStep(smart, "Apply the owner's annotations to both artifacts");
      },
    );
    loop.untilAll([cdk.condition.equals(shared.data.ownerAnswer, "approved")]);
  });
  shared.step.commit.commitStep("Commit the accepted walkthrough");

  return { receipts: shared.data.receipts };
}
