import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "../../../shared/agent.ts";
import { draftSlot, scopeSlot } from "../../../shared/data.ts";
import { target } from "../data.ts";

export const draftStage = cdk.stage({
    agent: smart1,
    input: cdk.input.prompt`
        Turn the scope ${scopeSlot} for ${target} into a concrete, actionable draft the worker can implement directly: exact files/areas and the precise change. Keep it to one bounded attempt.
    `,
    output: draftSlot,
});
