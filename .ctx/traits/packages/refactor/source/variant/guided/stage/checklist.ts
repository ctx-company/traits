import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "../../../shared/agent.ts";
import { plan } from "../../../shared/data.ts";
import { annotations } from "./annotate.ts";

export const checklistStage = cdk.stage({
    agent: smart1,
    input: cdk.input.prompt`Turn the captured annotations ${annotations} into a checklist.`,
    output: cdk.output.prompt`Return the checklist (${plan}).`,
});
