import * as cdk from "@ctx-traits/cdk";

import { baselineResult, bestMetric, decisions, history, keptCount, roundCount } from "#trait/shared/data.ts";
import { measureAggregateStep } from "#trait/shared/step/measure.ts";

export function measureBaselineStep(title: string, commandPort: cdk.PortHandle<string[]>) {
  return measureAggregateStep(title, commandPort, baselineResult);
}

export function seedBestStep(title: string) {
  return cdk.step.project(title, {
    id: "seed-best",
    projections: [
      { source: baselineResult, field: "metric", destination: bestMetric },
      { source: baselineResult, destination: history.with(cdk.operation.Append) },
      { source: cdk.operation.literal(0), destination: roundCount },
      { source: cdk.operation.literal(0), destination: keptCount },
      { source: cdk.operation.literal("baseline"), destination: decisions.with(cdk.operation.Append) },
    ],
  });
}
