import * as cdk from "@ctx-traits/cdk";

import { worker } from "#trait/shared/agent.ts";
import { draftSlot, implementReceipt } from "#trait/shared/data.ts";
import { target } from "../data.ts";

export const implementStep = cdk.step({
  agent: worker,
  input: cdk.input.prompt`
        Implement the draft ${draftSlot} for ${target} inside the isolated worktree.
        Do not run or report the benchmark; the runtime executes the trusted benchmark command next.
        Return a concise receipt naming the files changed.
    `,
  output: implementReceipt,
});
