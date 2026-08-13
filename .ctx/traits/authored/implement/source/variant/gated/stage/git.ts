import * as cdk from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { brief, plan, task, verdict, workSummary } from "../data.ts";

export const briefStage = cdk.stage({
  agent: agent.smart,
  input: cdk.input.prompt`
    The build loop for ${task} has ended in a double approval (reviewer and owner) and the work is being recorded and committed. Plan: ${plan}. Work summary: ${workSummary}. Final reviewer verdict: ${verdict}.
    Inspect the working tree with your tools (git diff, git status) so the record reflects the actual change, not just the summary's claims.
    Write: a short kebab-case slug for this task; the brief in markdown — what was built, why, how it was validated, and what the review found; and the commit message — a short subject line naming the change, then a one-paragraph summary of what was implemented and how it was validated.
    Return the slug, the markdown, and the commit message as the typed output. Do not write any file and do not run git commands; later runtime steps save and commit.`,
  output: brief,
});
