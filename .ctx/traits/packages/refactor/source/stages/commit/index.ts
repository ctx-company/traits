import type { PromptTemplate, SlotHandle } from "@ctx-traits/cdk";
import { input, output } from "@ctx-traits/cdk";

import { agreedDesign, commitMessage, survey, target } from "../../data.ts";

export function commitMessagePrompt(verdict1: SlotHandle, verdict2: SlotHandle): PromptTemplate {
    return input.prompt`
        The refinement loop for the refactor of ${target} has ended and the work is being committed. The final reviewer verdicts are ${verdict1} and ${verdict2} — read their status fields FIRST. The loop exits early once both approve, so a status still set to revise means the rounds ran out with that reviewer unsatisfied: say so plainly rather than implying a clean approval.
        Write the commit message from the agreed design ${agreedDesign}, the survey ${survey}, and those verdicts: subject line "refactor(${target}): <boundary in a few words>", then one paragraph summarizing the new boundary and behavior preservation, then a short "Deferred candidates:" list copied from the survey (omit the list if none were deferred).
        If either verdict status is revise, end the message with an "Unresolved reviewer blockers:" section listing each open blocker in one line — the honest record that the budget ran out before every reviewer was satisfied. Never write that section when there are none.
        Do not run any git commands and do not write any files for the message; staging and committing happen in later runtime steps.
    `;
}

export const commitMessageOutput = output.prompt`
    Return exactly the finished commit message — the runtime injects it into the git commit step (${commitMessage})
`;
