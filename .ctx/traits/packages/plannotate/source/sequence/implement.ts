import { condition, input, prompt, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { slot } from "../data.ts";

const implementWork = sequence.prompt("implement-work", {
    title: "Implement (worker)",
    agent: agent.worker,
    text: prompt.text`
                    Produce the work for the plan ${slot.plan}.
                    If no reviewer verdict is attached to this frame, this is round 1: implement the plan in full. If a verdict IS attached, this is a fix round: fix every BLOCKER it names — including any owner annotation it names as unsatisfied — and address advisory notes only when the fix is cheap and safe.
                    Run the project's own checks with your tools before reporting.
                    Return the full updated work summary — what changed (files), how you validated it, and any open concern — never a diff against your previous summary.`,
    output: slot.workSummary,
    // Neither `verdict` nor `ownerDecision` is interpolated above: both are
    // attached hidden-until-produced so round 1 (before either exists) does
    // not require them (pattern: implement-guided/source/sequence/implementation.ts).
    input: [input.optional(slot.verdict), input.optional(slot.ownerDecision)],
});

const implementReview = sequence.prompt("implement-review", {
    title: "Review the work (smart)",
    agent: agent.smart,
    text: prompt.text`
                    Review the implemented work for the plan ${slot.plan}. Current work summary: ${slot.workSummary}.
                    Inspect the actual working tree with your tools — never review the summary alone; re-run the project's checks if the summary does not prove they pass.
                    If an owner decision is attached to this frame, it is the owner's verdict on the previous round's briefing (denied or dismissed, since an approved verdict would already have ended the loop): treat any annotation it carries as an unsatisfied BLOCKER, never advisory, until the work visibly addresses it.
                    A BLOCKER makes the work genuinely unshippable: a correctness bug, a failing check, work the plan did not ask for, or an unsatisfied owner annotation. Everything else — naming, structure, taste, optional improvements — is ADVISORY.
                    Put blocking defects in blockers and non-blocking notes in advisory. Set status to revise only when blockers is non-empty; otherwise approved. Do not promote taste to a blocker to earn another round.
                    Also write briefing: a short markdown briefing of this round for the owner — what changed, what you found, and (only when status is approved) what you are asking the owner to confirm before it ships.
                    Return the typed verdict.`,
    output: slot.verdict,
    // `ownerDecision` is not interpolated above (referenced only in prose) —
    // same reason implement-work's optional inputs above are not
    // interpolated: an interpolated `${slot}` is an implicit required input,
    // which would demand a value round 1 never has.
    input: [input.optional(slot.ownerDecision)],
});

// Lifts review-verdict.briefing into a flat slot before the file-writing
// command step below: a command step's argv interpolates only local
// unqualified slot/port refs (`command_contract.rs`), never a nested field
// path — `project` is the runtime's own mechanism for pulling a nested field
// out to a flat slot (0085), and is what a flat-slot argv token can then
// reference. (The draft this trait was implemented from proposed an argv
// token straight from the nested field; the runtime's argv interpolation
// path does not resolve field paths, so it goes through `project` first —
// same pattern `summary-brief` already needs for `brief.slug`/`brief.markdown`.)
const liftBriefing = sequence.project("lift-owner-briefing", {
    projections: [{ source: slot.verdict, field: "briefing", destination: slot.ownerBriefing }],
});

const writeBriefing = sequence.command("write-owner-briefing", {
    title: "Write the round briefing for the owner",
    // `.git/CTX_BRIEFING.md` — plannotator's own file-name check requires
    // `/\.mdx?$/i`, which bars a stdin-style path like `/dev/stdin`.
    argv: ["sh", "-c", 'printf %s "$1" > .git/CTX_BRIEFING.md', "sh", "{slot:owner-briefing}"],
    output: slot.briefWriteOutput,
});

const implementOwnerReview = sequence.command("implement-owner-review", {
    title: "Summon the owner (plannotator, gated)",
    argv: ["plannotator", "annotate", ".git/CTX_BRIEFING.md", "--gate", "--json"],
    include: [slot.briefWriteOutput],
    // A human reads and decides in a browser window; no shorter bound fits.
    // Declared per-step (0084) so it wins over runtime.toml's
    // `command-idle-seconds = 600`.
    idleTimeoutMs: 4 * 60 * 60 * 1000,
    output: slot.ownerDecision,
});

// The owner is summoned only on a round the reviewer already approved — a
// `sequence.when`-style gate, authored as `sequence.branch` (the
// non-deprecated form) with only a `success` arm: an empty `failure`/
// otherwise arm is exactly the fallback the task's own risk list names if a
// `when`-gated step inside a loop body were ever rejected, so this is
// authored that way from the start rather than as a fallback.
const implementOwnerGate = sequence.branch("implement-owner-gate", {
    check: condition.fieldEquals(slot.verdict, "status", "approved"),
    success: [liftBriefing, writeBriefing, implementOwnerReview],
});

export default sequence.loop("implement-loop", {
    title: "Implement, review, and (on approval) summon the owner",
    sequence: sequence.linear("implement-round", [implementWork, implementReview, implementOwnerGate]),
    // No `iterations`/`maxIterations` (0093): this loop's thesis is that it
    // ends when the work is right — reviewer AND owner both approved — not
    // on a round count picked to approximate "never end". `onExhausted`
    // would be meaningless here and is refused by validation if declared.
    until: condition.all([
        condition.fieldEquals(slot.verdict, "status", "approved"),
        condition.fieldEquals(slot.ownerDecision, "decision", "approved"),
    ]),
});
