import { prompt, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, slot } from "../data.ts";

export default sequence.prompt("review", {
    title: "Review the result",
    agent: agent.reviewer,
    text: prompt.text`Review the implemented work for ${port.task} against the draft ${slot.draft}. The reported work summary is ${slot.workSummary}. Return a verdict of approved or revise, with notes explaining the verdict.`,
    output: slot.verdict,
    input: [port.task, slot.draft, slot.workSummary],
});
