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

export const commitMessageOutput = cdk.output.prompt`
    Return exactly the finished commit message — the runtime injects it into
    the git commit step (${commitMessageSlot})
`;
