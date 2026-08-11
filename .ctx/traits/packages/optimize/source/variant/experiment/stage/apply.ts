import * as cdk from "@ctx-traits/cdk";

import { worker } from "../../../shared/agent.ts";
import { applyReceipt, proposal } from "../../../shared/data.ts";
import { objective } from "../data.ts";

export const applyStage = cdk.stage({
    agent: worker,
    input: cdk.input.prompt`
        Apply exactly the bounded change in ${proposal} for ${objective} inside the isolated worktree.
        Inspect the current files first, preserve unrelated work, and do not run or report the experiment metric. The runtime executes the trusted measurement command next.
        Return a concise receipt naming the files changed.
    `,
    output: applyReceipt,
});
