// implement-quick: draft, then grind worker -> reviewer until approved, then
// commit. Ten rounds; exhausting them without approval blocks the run and
// commits nothing.
//
// DELIBERATELY LEAN (2026-07-27). This variant is called quick and now reads
// like it: no recurrence breaker, no earned-exit carry/min-rounds, no
// leftovers adjudication, no spliced doctrine blocks. Every one of those was
// a knob that interacted badly with the others — the measured failures were
// a breaker killing a converging loop, an earned-exit floor stacked on top
// of a fix loop, and prompts long enough that the actual instruction was
// buried. The loop IS the mechanism: the reviewer says "not yet" and the
// worker goes again, until it is satisfied or the budget is gone. The
// park-report projection (0047) is the one addition since: a deterministic,
// zero-prompt `project` step, never a spliced doctrine block, so it stays
// lean by this same rule.
import { condition, defineVariant, effect, flow, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

import { scribe } from "./agent.ts";
import { commitOutput, commitReport, parkReport, parkReportPort, verdict } from "./data.ts";
import * as stage from "./stage/index.ts";

export default function () {
  defineVariant("Quick", {
    name: "Implement (Quick)",
    description:
      "Quick dogfood implementation procedure: draft the approach from the plan, implement it, and grind a single reviewer loop until the work is approved — then commit.",
    metadata: { tag: [...shared.metadata.tag, "lean"] },
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
  });

  return { commitReport, parkReport: parkReportPort };
}
