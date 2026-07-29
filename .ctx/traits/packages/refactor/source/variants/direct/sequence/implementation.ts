import { prompt, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, slot } from "../data.ts";

export default sequence.prompt("implement", {
    title: "Implement the checklist",
    agent: agent.worker,
    text: prompt.text`Implement every item in the checklist ${slot.checklist}.`,
    output: port.workReport,
});
