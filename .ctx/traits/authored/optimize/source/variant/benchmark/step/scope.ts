import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "#trait/shared/agent.ts";
import { bestMetric, history, scopeSlot } from "#trait/shared/data.ts";
import { target } from "../data.ts";

export const scopeStep = cdk.step({
  agent: smart1,
  input: cdk.input.prompt`
        Scope one small, falsifiable attempt to improve the lower-is-better benchmark over ${target}.
        Current best metric is ${bestMetric}; trusted prior measurements are ${history}.
        Return the exact bounded area to touch and why it should improve the benchmark, staying inside ${target}.
    `,
  output: scopeSlot,
});
