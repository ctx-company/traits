// refactor-guarded's git steps: quick's verbatim, except the commit routes
// through `ctx-gate run --` and waits for the owner's approval before it
// lands (the implement:guarded convention, 0098). `env` carries the gate's
// state dir as plain argv tokens — command steps are literal argv, never a
// shell — and the path is deliberately hardcoded for now.
import * as cdk from "@ctx-traits/cdk";

import { slot } from "#trait/shared/data.ts";

export * from "#trait/shared/step/git.ts";

export const commitSubmit = cdk.defineStep.command({
  input: cdk.input
    .command`env CTX_GATE_STATE_DIR=/Users/rpunkfu/ctx-company/ctx-gate/.ctx/gate/state ctx-gate run -- git commit -m ${slot.commitMessage}`,
  output: slot.commitOutput,
  timeoutMs: 28_800_000,
});
