import * as cdk from "@ctx-traits/cdk";

import { baselineResult, bestMetric, decisions, history, keptCount, roundCount } from "../data.ts";
import { measureAggregateStage } from "./measure.ts";

export function measureBaselineStage(title: string, commandPort: cdk.PortHandle<string[]>) {
  return measureAggregateStage(title, commandPort, baselineResult);
}

export function seedBestStage(title: string) {
  return cdk.step.project(title, {
    id: "seed-best",
    projections: [
      { source: baselineResult, field: "metric", destination: bestMetric },
      { source: baselineResult, destination: cdk.operation.over(history, cdk.operation.Append) },
      { source: cdk.operation.literal(0), destination: roundCount },
      { source: cdk.operation.literal(0), destination: keptCount },
      { source: cdk.operation.literal("baseline"), destination: cdk.operation.over(decisions, cdk.operation.Append) },
    ],
  });
}
