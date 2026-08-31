import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  cdk.defineVariant("Default", {
    description:
      "Research a feature idea online, ground it in this codebase, and refine a two-artifact proposal (markdown + HTML diagram) until the owner accepts it through ctx-annotate.",
    metadata: { tag: shared.metadata.tag },
  });

  const smart = shared.agent.smart(
    "Researches online, grounds in the codebase, composes both artifacts, and applies the owner's annotations.",
    "Research + composition + revision role.",
  );

  shared.step.slugify.slugStep("Derive the artifact slug");
  shared.step.compose.composeStep(smart, "Research and compose the proposal");
  // Owner acceptance: effectively endless — the ceiling is the owner's
  // empty-annotations acceptance; 500 is a runaway backstop only. The
  // gate opens the diagram and pipes the markdown to ctx-annotate; any
  // annotations become binding corrections applied to BOTH artifacts
  // before the next gate iteration.
  cdk.flow.loop("Owner acceptance", (loop) => {
    loop.maxIterations(500, { onExhausted: cdk.signal.Abort });
    shared.step.gate.acceptanceGate("Present the proposal to the owner");
    cdk.flow.when(
      "Apply owner annotations",
      cdk.condition.not(cdk.condition.equals(shared.data.ownerAnswer, "approved")),
      () => {
        shared.step.compose.reviseStep(smart, "Apply the owner's annotations to both artifacts");
      },
    );
    loop.untilAll([cdk.condition.equals(shared.data.ownerAnswer, "approved")]);
  });
  shared.step.commit.commitStep("Commit the accepted brainstorm");

  return { receipts: shared.data.receipts };
}
