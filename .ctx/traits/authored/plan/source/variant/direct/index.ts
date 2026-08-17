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
      "Derive the next board key and date, then format the described work near-verbatim into one TaskDocument TOML file on the board.",
  });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);

  const smart1 = shared.agent.smart1(
    "Formats the described work into one well-formed board task file, near-verbatim.",
    "Format-and-write role.",
  );

  shared.stage.derive.nextKeyStep();
  shared.stage.derive.raisedDateStep();
  shared.stage.distill.verbatim(smart1);

  return { writtenFiles: shared.data.writtenFiles };
}
