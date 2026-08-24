// plan-default: ingest the source's own contract (work items, done
// criteria), ground the work in the codebase, split it into a typed slice
// plan with final board keys assigned once from the derived next-free key,
// then write each slice's TaskDocument TOML files in its own for-each frame
// — the composing seat writes its own deliverable, there is no
// transcription hop — and return the typed receipts.
import { defineVariant, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Default", {
    name: "Plan",
    summary:
      "Turn described work into board-ready TaskDocument TOML task files in .internal/tasks/ — duration-targeted tasks (default 10-15 minutes) shaped as the work demands — bare tasks or charters with children — with typed relations, the format the board dispatch machinery actually resolves.",
    metadata: { tag: shared.metadata.tag },
    description:
      "Turn described work into board-ready TaskDocument TOML task files in .internal/tasks/ — duration-targeted tasks (default 10-15 minutes) shaped as the work demands — bare tasks or charters with children — with typed relations, the format the board dispatch machinery actually resolves.",
    procedureDescription:
      "Extract the source's work items and done criteria, ground the work in the codebase, split it into a symbolic-keyed typed slice plan shaped as the work demands, write each slice's TaskDocument TOML files in its own frame, and assign final board keys mechanically from the live board.",
  });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);

  const smart1 = shared.agent.smart1(
    "Strong model: extracts the source contract, grounds the work in the codebase, plans the slices, and writes each slice's task files in its own frame.",
    "Contract + grounding + planning + composition role.",
  );

  shared.step.derive.boardSnapshotStep();
  shared.step.derive.raisedDateStep();
  shared.step.ingest.contract(smart1);
  shared.step.refine.task(smart1);
  shared.step.split.slices(smart1);
  shared.step.writeSlices.tasks(smart1, shared.step.split.MAX_SLICES);
  shared.step.renumber.finalKeysStep();

  return { writtenFiles: shared.data.writtenFiles, finalKeys: shared.data.finalKeys };
}
