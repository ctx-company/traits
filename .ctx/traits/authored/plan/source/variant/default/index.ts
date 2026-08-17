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
      "Turn described work into board-ready TaskDocument TOML task files in .internal/tasks/ — charters plus 10-15-minute children with typed relations, the format the board dispatch machinery actually resolves.",
    metadata: { tag: shared.metadata.tag },
    description:
      "Turn described work into board-ready TaskDocument TOML task files in .internal/tasks/ — charters plus 10-15-minute children with typed relations, the format the board dispatch machinery actually resolves.",
    procedureDescription:
      "Extract the source's work items and done criteria, ground the work in the codebase, split it into a typed slice plan with final keys, write each slice's TaskDocument TOML files in its own frame, and return the receipts.",
  });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);

  const smart1 = shared.agent.smart1(
    "Strong model: extracts the source contract, grounds the work in the codebase, plans the slices, and writes each slice's task files in its own frame.",
    "Contract + grounding + planning + composition role.",
  );

  shared.stage.derive.nextKeyStep();
  shared.stage.derive.raisedDateStep();
  shared.stage.ingest.contract(smart1);
  shared.stage.refine.task(smart1);
  shared.stage.split.slices(smart1);
  shared.stage.writeSlices.tasks(smart1, 6);

  return { writtenFiles: shared.data.writtenFiles };
}
