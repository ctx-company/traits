import { condition, input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { slot } from "../data.ts";

export default sequence.loop("building", {
    title: "Implement and review",
    sequence: sequence.linear("build-round", [
        sequence.prompt("implement", {
            title: "Implement (worker)",
            agent: agent.worker,
            text: input.prompt`
                            Produce the work for the annotated assignment ${slot.annotations} against the draft ${slot.draft}.
                            If no reviewer verdict is attached to this frame, this is round 1: implement the draft in full. If a verdict IS attached, this is a fix round: fix every BLOCKER it names, and address advisory notes only when the fix is cheap and safe.
                            Run the project's own checks with your tools before reporting.
                            Return the full updated work summary — what changed (files), how you validated it, and any open concern — never a diff against your previous summary.`,
            output: slot.workSummary,
            // `verdict` is NOT interpolated above: it is attached hidden-until-produced
            // so round 1 (before any review exists) does not require it.
            input: [input.optional(slot.verdict)],
        }),
        sequence.prompt("review", {
            title: "Review the work (smart)",
            agent: agent.smart,
            text: input.prompt`
                            Review the implemented work for the annotated assignment ${slot.annotations} against the draft ${slot.draft}. Current work summary: ${slot.workSummary}.
                            Inspect the actual working tree with your tools — never review the summary alone; re-run the project's checks if the summary does not prove they pass.
                            A BLOCKER makes the work genuinely unshippable: a correctness bug, a failing check, or work the annotations did not ask for. Everything else — naming, structure, taste, optional improvements — is ADVISORY.
                            Put blocking defects in blockers and non-blocking notes in advisory. Set status to revise only when blockers is non-empty; otherwise approved. Do not promote taste to a blocker to earn another round.
                            Return the typed verdict.`,
            output: slot.verdict,
        }),
    ]),
    until: condition.fieldEquals(slot.verdict, "status", "approved"),
    iterations: 6,
    // A run that spends its rounds without an approving verdict has not been
    // reviewed clean, and must not reach the walkthrough or the commit tail.
    onExhausted: "abort",
});
