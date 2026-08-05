import { input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { slot } from "../data.ts";

export default sequence.prompt("draft", {
    title: "Draft the work (smart)",
    agent: agent.smart,
    text: input.prompt`
                    Create an implementation draft for the annotated assignment ${slot.annotations}.
                    Every entry in its annotations list is part of the scope — implement exactly what they ask, together, and nothing more. Each annotation's "text" is the request; its "lines" locate it in the annotated source and are context, not work.
                    Inspect the repository with your tools to ground the draft; do not guess at structure.
                    Cover: scope, the files to touch, the approach, the validation plan (which of the project's own checks prove it), and the risks.
                    Reference files by path; do not inline documents. Do not implement anything yet.`,
    output: slot.draft,
});
