import * as cdk from "@ctx-traits/cdk";

import { draft, verdict } from "../data.ts";

export const scribeText = cdk.input.prompt`The review has ended approved and the work is being committed.
    Write a concise commit message from the draft ${draft} and the verdict ${verdict}: a short subject line naming the change, then one paragraph on what was implemented and how it was validated.
    Return exactly that message. Do not run git commands and do not write files.`;
