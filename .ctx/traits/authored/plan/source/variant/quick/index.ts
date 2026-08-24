// plan-quick: one grounded frame distills the described work straight into
// board TaskDocument TOML files — the composing seat writes its own
// deliverable — with only the numbering and date derived deterministically
// first. No separate grounding pass, no review pass.
import { defineVariant, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Quick", {
    name: "Plan (Quick)",
    summary:
      "Distill described work straight into board-ready TaskDocument TOML task files — no separate grounding pass, no review pass.",
    metadata: { tag: shared.metadata.tag },
    description:
      "Distill described work straight into board-ready TaskDocument TOML task files — no separate grounding pass, no review pass.",
    procedureDescription:
      "Distill the described work — grounded by the frame's own repository reading — straight into symbolic-keyed TaskDocument TOML files on the board, then assign final board keys mechanically from the live board.",
  });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);

  const smart1 = shared.agent.smart1(
    "Distills the described work directly into board task files, grounded by its own repository reading.",
    "Distill-and-write role.",
  );

  shared.step.derive.raisedDateStep();
  shared.step.distill.tasks(smart1);
  shared.step.renumber.finalKeysStep();
  shared.step.commit.boardCommitStep();

  return { writtenFiles: shared.data.writtenFiles, finalKeys: shared.data.finalKeys };
}
