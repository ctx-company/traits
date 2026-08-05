import { condition, input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { slot } from "../data.ts";

const implementWork = sequence.prompt("implement-work", {
    title: "Implement (worker)",
    agent: agent.worker,
    text: input.prompt`
                    Produce the work for the plan ${slot.plan}.
                    If no reviewer verdict is attached to this frame, this is round 1: implement the plan in full. If a verdict IS attached, this is a fix round: fix every BLOCKER it names — including any owner annotation it names as unsatisfied — and address advisory notes only when the fix is cheap and safe.
                    Run the project's own checks with your tools before reporting.
                    Return the full updated work summary — what changed (files), how you validated it, and any open concern — never a diff against your previous summary.`,
    output: slot.workSummary,
    // Attached hidden-until-produced so round 1 (before either exists) does
    // not require them; an interpolated slot would be an implicit required input.
    input: [input.optional(slot.verdict), input.optional(slot.ownerDecision)],
});

const implementReview = sequence.prompt("implement-review", {
    title: "Review the work (smart)",
    agent: agent.smart1,
    text: input.prompt`
                    Review the implemented work for the plan ${slot.plan}. Current work summary: ${slot.workSummary}.
                    Inspect the actual working tree with your tools — never review the summary alone; re-run the project's checks if the summary does not prove they pass.
                    If an owner decision is attached to this frame, it is the owner's verdict on the previous round's briefing (denied or dismissed, since an approved verdict would already have ended the loop): treat any annotation it carries as an unsatisfied BLOCKER, never advisory, until the work visibly addresses it.
                    A BLOCKER makes the work genuinely unshippable: a correctness bug, a failing check, work the plan did not ask for, or an unsatisfied owner annotation. Everything else — naming, structure, taste, optional improvements — is ADVISORY.
                    Put blocking defects in blockers and non-blocking notes in advisory. Set status to revise only when blockers is non-empty; otherwise approved. Do not promote taste to a blocker to earn another round.
                    Return the typed verdict.`,
    output: slot.verdict,
    input: [input.optional(slot.ownerDecision)],
});

const roundBriefing = sequence.prompt("round-briefing", {
    title: "Write the round briefing (smart)",
    agent: agent.smart1,
    text: input.prompt`
                    The reviewer has approved this round's work for the plan ${slot.plan}, and the owner is about to be summoned to confirm it ships. Work summary: ${slot.workSummary}. Reviewer verdict: ${slot.verdict}.
                    Inspect the working tree with your tools (git diff, git status, the changed files) so the briefing reflects what actually changed, not just what the summary claims.
                    Write a short markdown briefing for the owner, to be read cold — the owner has not seen the working tree: what changed and why, what the review checked and found (including advisory notes worth the owner's attention), and what the owner is being asked to confirm.
                    Return only the briefing markdown.`,
    output: slot.ownerBriefing,
});

// plannotator's `annotate` requires a real `.md` file path (no stdin form),
// so the briefing goes through a transient temp file created and removed
// inside this one step; plannotator's stdout is the only stdout, so the
// gate JSON lands in the output slot clean.
const implementOwnerReview = sequence.command("implement-owner-review", {
    title: "Summon the owner (plannotator, gated)",
    argv: [
        "sh",
        "-c",
        'd=$(mktemp -d) && printf %s "$1" > "$d/briefing.md" && plannotator annotate "$d/briefing.md" --gate --json; s=$?; rm -rf "$d"; exit $s',
        "sh",
        "{slot:owner-briefing}",
    ],
    // A human reads and decides in a browser; the per-step bound must beat
    // runtime.toml's `command-idle-seconds`.
    idleTimeoutMs: 4 * 60 * 60 * 1000,
    output: slot.ownerDecision,
});

// The owner is summoned only on a round the reviewer already approved.
const implementOwnerGate = sequence.branch("implement-owner-gate", {
    check: condition.fieldEquals(slot.verdict, "status", "approved"),
    success: [roundBriefing, implementOwnerReview],
});

export default sequence.loop("implement-loop", {
    title: "Implement, review, and (on approval) summon the owner",
    sequence: sequence.linear("implement-round", [implementWork, implementReview, implementOwnerGate]),
    // No iteration bound by design: the loop ends when the work is right —
    // reviewer AND owner both approved — not on a round count.
    until: condition.all([
        condition.fieldEquals(slot.verdict, "status", "approved"),
        condition.fieldEquals(slot.ownerDecision, "decision", "approved"),
    ]),
});
