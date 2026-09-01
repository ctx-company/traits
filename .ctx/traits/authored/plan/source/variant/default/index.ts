// plan-default: ingest the source's own contract (work items, done
// criteria), ground the work in the codebase, split it into a typed slice
// plan with final board keys assigned once from the derived next-free key,
// then write each slice's TaskDocument TOML files in its own for-each frame
// — the composing seat writes its own deliverable, there is no
// transcription hop — and return the typed receipts.
import * as cdk from "@ctx-traits/cdk";
import { defineVariant, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Default", {
    name: "Plan",
    summary:
      "Turn described work into ONE board-ready parent TaskDocument in .internal/tasks/ — intent and acceptance shape with typed relations; decomposition belongs to architect (owner ruling 2026-09-01).",
    metadata: { tag: shared.metadata.tag },
    description:
      "Turn described work into ONE board-ready parent TaskDocument in .internal/tasks/ — intent and acceptance shape with typed relations; decomposition belongs to architect (owner ruling 2026-09-01).",
    procedureDescription:
      "Extract the source's work items and done criteria, ground the work in the codebase, shape it as one symbolic-keyed parent slice, write that parent's TaskDocument in one frame, run one bounded independent review pass, then assign final board keys mechanically from the live board and commit the written files.",
  });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);

  const smart1 = shared.agent.smart1(
    "Strong model: extracts the source contract, grounds the work in the codebase, plans the slices, writes each slice's task files in its own frame, and applies review fixes.",
    "Contract + grounding + planning + composition + revision role.",
  );
  const smart2 = shared.agent.smart2(
    "Independent strong model: one bounded review pass of the written board, separately from smart-1.",
    "Simple review role.",
  );

  shared.step.derive.boardSnapshotStep();
  shared.step.derive.raisedDateStep();
  shared.step.ingest.contract(smart1);
  shared.step.refine.task(smart1);
  shared.step.split.slices(smart1);
  shared.step.writeSlices.tasks(smart1, shared.step.split.MAX_SLICES);
  shared.step.review.simple(smart2, smart1);
  shared.step.renumber.finalKeysStep();
  // Owner acceptance (2026-08-31): after final keys, the written board is
  // piped to the owner's ctx-annotate; empty annotations accept and the
  // board commits, anything else becomes binding corrections applied by
  // the composing seat before the next gate iteration. Effectively
  // endless — 500 is a runaway backstop, the ceiling is acceptance.
  cdk.flow.loop("Owner acceptance", (loop) => {
    loop.maxIterations(500, { onExhausted: cdk.signal.Abort });
    shared.step.owner.acceptanceGate("Present the board to the owner");
    cdk.flow.when(
      "Apply owner corrections",
      cdk.condition.not(cdk.condition.equals(shared.data.ownerAnswer, "approved")),
      () => {
        shared.step.owner.applyCorrections(smart1, "Apply the owner's annotations");
      },
    );
    loop.untilAll([cdk.condition.equals(shared.data.ownerAnswer, "approved")]);
  });
  shared.step.commit.boardCommitStep();

  return { writtenFiles: shared.data.writtenFiles, finalKeys: shared.data.finalKeys };
}
