import { input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, slot } from "../data.ts";

export default sequence.prompt("draft", {
    title: "Draft the approach",
    agent: agent.planner,
    input: input.text`Draft an implementation approach for ${port.task}. Cover scope, the concrete approach, files or areas likely to change, and how the result will be validated. Do not implement anything yet.`,
    output: slot.draft,
});
