import * as cdk from "@ctx-traits/cdk";

import { task, workSummary } from "../data.ts";

export const scribeText = cdk.input.prompt`The work for ${task} is being committed.
    Write a concise commit message from the work summary ${workSummary}: a short subject line naming the change, then one paragraph on what was implemented and how it was validated.
    Return exactly that message. Do not run git commands and do not write files.`;
