import { prompt, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, slot } from "../data.ts";

const planDraft = sequence.prompt("plan-draft", {
    title: "Draft the plan (smart)",
    agent: agent.smart,
    text: prompt.text`
                    Create an implementation plan for the assignment: ${port.assignment}.
                    Inspect the repository with your tools to ground the plan; do not guess at structure.
                    Cover: scope, the files to touch, the approach, the validation plan (which of the project's own checks prove it), and the risks.
                    Reference files by path; do not inline documents. Do not implement anything yet.`,
    output: slot.draft,
});

// A command step has no stdin channel — every spawn is `Stdio::null()` — so
// piping the hook envelope into plannotator rides `sh -c`, the same
// workaround `implement-guided/source/sequence/annotation.ts` documents. The
// draft text reaches the shell as a positional `$1` argument, never spliced
// into the script string, so no character in the plan can break out of the
// intended command. `PLANNOTATOR_ORIGIN` is set deliberately so a deny reads
// as addressed to plannotator, not to some other tool sharing the hook shape.
//
// The exact `jq` envelope below encodes the shape this trait's own schema
// (`schema.planDecision`) and the core fixture at
// `modules/core/src/procedure/session.rs:4374` agree on —
// `hookSpecificOutput.decision.behavior`/`.message` — but was authored
// without a live plannotator session to confirm against (interactive; opens
// a browser and blocks on a human decision, which this environment could not
// drive). Confirm against a real captured payload before recording.
const planOwnerReview = sequence.command("plan-owner-review", {
    title: "Annotate the plan (plannotator, plan mode)",
    argv: [
        "sh",
        "-c",
        'printf %s "$1" | jq -n --arg plan "$(cat)" \'{hook_event_name: "PreToolUse", tool_name: "ExitPlanMode", tool_input: {plan: $plan}}\' | PLANNOTATOR_ORIGIN=plannotate-trait plannotator',
        "sh",
        "{slot:draft}",
    ],
    // A human annotates the plan in a browser window; there is no
    // "silence budget" shorter than that. `.ctx/traits/runtime.toml` sets
    // `command-idle-seconds = 600`; this step declares its own bound (0084)
    // so it wins over that default rather than timing out mid-review.
    idleTimeoutMs: 4 * 60 * 60 * 1000,
    output: slot.planDecision,
});

const planReview = sequence.prompt("plan-review", {
    title: "Refine the plan against the owner's annotations (smart)",
    agent: agent.smart,
    text: prompt.text`
                    Refine the draft ${slot.draft} into the final plan, using plannotator's plan-mode verdict ${slot.planDecision}.
                    If hookSpecificOutput.decision.behavior is "approve", the refined plan may be the draft unchanged.
                    If it is "deny", hookSpecificOutput.decision.message is the owner's annotation — every point it raises must be visibly and provably addressed in the refined plan (not merely acknowledged): change the affected section, or state explicitly why the draft already covers it.
                    Reference files by path; do not inline documents. Do not implement anything yet.`,
    output: slot.plan,
});

export default [planDraft, planOwnerReview, planReview];
