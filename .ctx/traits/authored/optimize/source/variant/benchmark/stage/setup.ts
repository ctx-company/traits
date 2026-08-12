import * as cdk from "@ctx-traits/cdk";

import { worker } from "../../../shared/agent.ts";
import { readinessSlot } from "../../../shared/data.ts";
import { target } from "../data.ts";

export const setupStage = cdk.stage({
    agent: worker,
    input: cdk.input.prompt`
        Prepare the workbench for ${target}. Confirm this execution is inside an isolated Git worktree and inspect the current tracked state.
        Return status ready only when the tracked workbench is clean and destructive reset is confined to that worktree. Otherwise return abort with the concrete reason.
        Do not modify files yet and do not invent metric evidence.
    `,
    output: readinessSlot,
});
