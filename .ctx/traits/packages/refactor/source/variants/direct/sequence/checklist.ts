import { prompt, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { slot } from "../data.ts";

export default sequence.prompt("checklist", {
    title: "Build the checklist",
    agent: agent.smart1,
    text: prompt.text`Turn the captured annotations ${slot.annotations} into a checklist.`,
    output: slot.checklist,
});
