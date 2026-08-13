import { condition, defineVariant, effect, flow, operation, step, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

import { scribe } from "./agent.ts";
import { buildingSealed, parkReport, parkReportPort, planningVerdict, verdict1, verdict2 } from "./data.ts";
import * as stage from "./stage/index.ts";

export default function () {
  defineVariant("Smart", {
    name: "Implement (Smart)",
    description:
      "Research-informed dogfood implementation procedure: research prior art, extract the task contract from the task board, draft the approach, implement it, refine against two independent reviewers who may amend the draft, and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "multi-agent"] },
  });

  useBehavior(shared.metadata.FAMILY_BEHAVIOR);
  useIntent(shared.intent.FAMILY_INTENT);

  stage.frame.extract();
  // DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
  // feasibilityStep(smart1);

  stage.plan.research("Research prior art (smart-1)", { id: "research" });

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

    shared.deriveParkReportStep([verdict1, verdict2], { parkReportSlot: parkReport });

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

  return { commitReport: shared.data.commitReport, leftovers: shared.data.leftoversPort, parkReport: parkReportPort };
}
