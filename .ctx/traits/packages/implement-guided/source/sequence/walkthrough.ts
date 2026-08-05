import { input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { slot } from "../data.ts";

const captureDiff = sequence.command("capture-diff", {
    title: "Capture the approved diff",
    // Same runtime-state exclusion the staging step uses, so the walkthrough
    // never describes the run's own ledger as part of the change.
    argv: ["git", "diff", "HEAD", "--", ".", ":(exclude).agents/runs"],
    output: slot.reviewedDiff,
});

const explainHunks = sequence.prompt("explain-hunks", {
    title: "Explain the change hunk by hunk (smart)",
    agent: agent.smart,
    text: input.prompt`
                    You have just approved this work. Now write a markdown walkthrough of it for the human who will read the change.
                    ${slot.reviewedDiff} is the approved change as unified-diff hunks.
                    Return MARKDOWN, not a data structure. One "##" section per hunk, in the order the diff presents them, each section titled with the file path and the hunk's @@ header. Inside each section: first the hunk itself verbatim inside a fenced code block opened and closed by three backtick characters, with the language tag diff on the opening fence; then a bold **Intent:** line stating in one sentence what the change is FOR; then an **Explanation:** paragraph covering how it achieves that intent, the assumption it relies on, and any case it deliberately does not handle.
                    One section per hunk — never merge two hunks, never invent a hunk the diff does not contain, and never describe the syntax instead of the purpose.
                    Write for someone who has not read the annotations ${slot.annotations} or the draft ${slot.draft}. Return only the markdown.`,
    output: slot.hunkNotes,
});

export default [captureDiff, explainHunks];
