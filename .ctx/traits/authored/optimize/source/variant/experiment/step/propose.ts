import * as cdk from "@ctx-traits/cdk";

import { proposer } from "#trait/shared/agent.ts";
import { bestMetric, history, proposal } from "#trait/shared/data.ts";
import { metricField, objective } from "../data.ts";

export const proposeStep = cdk.step({
  agent: proposer,
  input: cdk.input.prompt`
        Propose one small, falsifiable experiment for ${objective}.
        The metric being optimized is ${metricField}; lower is better.
        Current best metric is ${bestMetric}; trusted prior measurements are ${history}.
        Use only this typed handoff. Do not infer hidden conversation, claim a result, or decide keep/discard.
        Return a title, hypothesis, and exact bounded change.
    `,
  output: proposal,
});
