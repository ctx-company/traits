import * as cdk from "@ctx-traits/cdk";

import { commitMessage as commitMessageSlot, commitOutput, gitStatus, stageOutput } from "../data.ts";

export const status = cdk.stage({
    input: cdk.input.command`git status --porcelain`,
    output: gitStatus,
});

export const commitStage = cdk.stage({
    input: cdk.input.command`git add -A`,
    output: stageOutput,
});

export const commitSubmit = cdk.stage({
    input: cdk.input.command`git commit -m ${commitMessageSlot}`,
    output: commitOutput,
});

// Every variant flavors the commit-message INPUT (which verdicts, which
// record it writes from), so the prompt lives in each variant's local
// stage/git.ts; the return contract is the shared half.
export const commitMessageOutput = cdk.output.prompt`
    Return exactly the finished commit message — the runtime injects it into
    the git commit step (${commitMessageSlot})
`;
