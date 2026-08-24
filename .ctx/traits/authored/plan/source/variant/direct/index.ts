// plan-direct: format the description near-verbatim into exactly one
// board TaskDocument TOML file — no refinement, no invention, no repository
// reading; the fastest path to just implement.
import { defineVariant, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Direct", {
    name: "Plan (Direct)",
    summary:
      "Format described work into one well-formed board TaskDocument TOML file with wording kept near-verbatim — no refinement, no invention, the fastest path to just implement.",
    metadata: { tag: shared.metadata.tag },
    description:
      "Format described work into one well-formed board TaskDocument TOML file with wording kept near-verbatim — no refinement, no invention, the fastest path to just implement.",
    procedureDescription:
      "Format the described work near-verbatim into one symbolic-keyed TaskDocument TOML file on the board, then assign its final board key mechanically from the live board.",
  });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);

  const smart1 = shared.agent.smart1(
    "Formats the described work into one well-formed board task file, near-verbatim.",
    "Format-and-write role.",
  );

  shared.step.derive.raisedDateStep();
  shared.step.distill.verbatim(smart1);
  shared.step.renumber.finalKeysStep();
  shared.step.commit.boardCommitStep();

  return { writtenFiles: shared.data.writtenFiles, finalKeys: shared.data.finalKeys };
}
