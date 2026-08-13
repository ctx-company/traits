// implement-guarded (0098): the quick loop, unchanged, except the final
// commit routes through `ctx-gate run --` and waits for the owner's
// approval before it lands. Not to be confused with `gated` (the existing
// plannotator-based variant) — "guarded" here means an approval gate stands
// between the work and the commit, and this variant adds the owner as one
// more approver on top of the two model reviewers.
import { condition, defineVariant, effect, flow, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import { scribe } from "#trait/variant/quick/agent.ts";
import { commitOutput, commitReport, parkReport, parkReportPort, verdict } from "#trait/variant/quick/data.ts";
import * as stage from "#trait/variant/quick/stage/index.ts";

export default function () {
  defineVariant("Guarded", {
    name: "Implement (Guarded)",
    description:
      "Quick dogfood implementation procedure, with the final commit held for the owner's approval via ctx-gate before it lands.",
    metadata: { tag: [...shared.metadata.tag, "lean", "gated"] },
  });

  useBehavior(shared.metadata.FAMILY_BEHAVIOR);
  useIntent(shared.intent.QUICK_INTENT);

  stage.draft.compose("Draft the work");

  flow.loop("Building", (loop) => {
    loop.maxIterations(10, { onExhausted: "abort" });

    stage.build.apply("Building Produce");

    shared.gateAndDiffEvidence({ gatePassed: shared.data.repoGatesPassed, diff: shared.data.reviewDiff });

    stage.build.review("Building Review");

    shared.deriveParkReportStep(verdict, { parkReportSlot: parkReport });

    flow.when("Gate Timed Out", shared.data.gateTimedOutAbortIf, flow.Abort);
    effect.onAbort(shared.data.gateTimedOut);

    flow.until(
      condition.all([
        condition.equals(verdict.status, "approved"),
        condition.equals(shared.data.repoGatesPassed.ok, true),
      ]),
    );
  });

  shared.familyCommitTail({
    id: "shipping",
    scribe: { agent: scribe, text: stage.git.scribeText },
    receipt: commitOutput,
    gate: { prefix: "ctx-gate run --", title: "Commit the work (awaiting ctx-gate approval)", timeoutMs: 28_800_000 },
  });

  return { commitReport, parkReport: parkReportPort };
}
