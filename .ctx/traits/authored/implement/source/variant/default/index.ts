import { condition, defineVariant, effect, flow, step, useBehavior, useIntent } from "@ctx-traits/cdk";
import { operation } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

import { scribe } from "./agent.ts";
import { buildingSealed, planningVerdict, verdict1, verdict2 } from "./data.ts";
import * as stage from "./stage/index.ts";

export default function () {
  defineVariant("Default", {
    name: "Implement (Default)",
    description:
      "Surveyed dogfood implementation procedure: extract the task contract from the task board, draft the approach, implement it, then a doubly-reviewed bounded refinement loop — then summarize and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "multi-agent"] },
  });

  useBehavior(shared.metadata.FAMILY_BEHAVIOR);
  useIntent(shared.intent.FAMILY_INTENT);

  stage.frame.extract();

  flow.loop("Planning", (loop) => {
    loop.maxIterations(2, { onExhausted: "continue" });

    stage.plan.produce("Planning Produce");
    stage.plan.review("Planning Review");

    flow.until(condition.equals(planningVerdict.status, "approved"));
  });

  flow.loop("Building", (loop) => {
    loop.maxIterations(5, { onExhausted: "abort" });

    step.project("Building Unseal", { projections: [{ source: operation.literal(false), destination: buildingSealed }] });

    stage.build.produce("Building Produce");

    shared.gateAndDiffEvidence({ gatePassed: shared.data.repoGatesPassed, diff: shared.data.reviewDiff });

    stage.build.review1("Building Review 1");
    stage.build.review2("Building Review 2");

    shared.deriveParkReportStep([verdict1, verdict2], { parkReportSlot: shared.data.parkReport });

    step.project("Building Seal", { projections: [{ source: operation.literal(true), destination: buildingSealed }] });

    flow.when("Gate Timed Out", shared.data.gateTimedOutAbortIf, flow.Abort);
    effect.onAbort(shared.data.gateTimedOut);

    flow.until(
      condition.all([
        condition.all([condition.equals(verdict1.status, "approved"), condition.equals(verdict2.status, "approved")]),
        condition.equals(shared.data.repoGatesPassed.ok, true),
        condition.equals(buildingSealed, true),
        condition.count(shared.data.leftovers).where("needs", []).equals(0),
        condition.any([condition.count(shared.data.leftovers).equals(0), condition.iterationAtLeast(2)]),
      ]),
    );
  });

  shared.familyCommitTail({
    id: "shipping",
    scribe: { agent: scribe, text: stage.git.scribeText },
    receipt: shared.data.commitOutput,
  });

  return { commitReport: shared.data.commitReport, leftovers: shared.data.leftoversPort, parkReport: shared.data.parkReportPort };
}
