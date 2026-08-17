// implement-quick: one worker pass, then commit. No reviewer, no gates, no
// park — the leanest path from a board task to a commit (owner ruling
// 2026-08-17). Use default or complex when the work deserves review.
import { defineVariant, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Quick", {
    name: "Implement (Quick)",
    description:
      "Single-pass implementation: the worker implements the board task in full — honoring its Watch and Done when — and the result is committed. No review loop.",
    metadata: { tag: [...shared.metadata.tag, "lean"] },
  });
  useBehavior(shared.metadata.FAMILY_BEHAVIOR);
  useIntent(shared.intent.QUICK_INTENT);

  shared.stage.implement.apply("Implement the task");

  shared.familyCommitTail({
    id: "shipping",
    scribe: { agent: shared.agent.scribe, text: shared.stage.git.scribeText },
    receipt: shared.data.commitOutput,
  });

  return { commitReport: shared.data.commitReport };
}
