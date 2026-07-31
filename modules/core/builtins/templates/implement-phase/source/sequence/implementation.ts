import { input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, slot } from "../data.ts";

export default sequence.prompt("implement", {
    title: "Implement the draft",
    agent: agent.worker,
    input: input.text`Implement ${port.task} following the draft ${slot.draft}. Prefer the minimal, correct change the draft calls for. Report what changed and how you validated it.`,
    output: slot.workSummary,
});
