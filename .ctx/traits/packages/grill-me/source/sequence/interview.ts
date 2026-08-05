import { condition, input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, resource, slot } from "../data.ts";

const probe = sequence.prompt("probe", {
    title: "Ask the next question (smart-1)",
    agent: agent.interrogator,
    text: input.prompt`
                    Grill the plan ${port.plan}: a relentless interview that sharpens it until nothing vague enough to build wrong remains. Hold to the doctrine ${resource.doctrine}.
                    If the plan names a repository file, read it with your tools first. If an interview ledger is attached to this frame, every entry in it is settled — never re-ask one; on round 1 no ledger exists yet.
                    Walk the plan's decision tree in dependency order — a parent decision before the choices that hang off it — and pick the SINGLE most load-bearing unresolved point. Ask exactly one question about it: name the fork, why it matters, and what choosing wrong would cost. Attach your recommended answer with a one-line rationale — a question is a proposal to react to, never a blank prompt.
                    Classify it: kind is fact when the repository or environment can settle it, decision when it is genuinely the owner's call.
                    Set status to exhausted — omitting question, kind, and recommendation — only when every branch is settled in the ledger or already queued for the owner. Otherwise status is continue. Return the typed probe.`,
    output: slot.probe,
    // `ledger` is not interpolated above (referenced only in prose): an
    // interpolated `${slot}` is an implicit required input, which would
    // demand a value round 1 never has (pattern:
    // plannotate/source/sequence/implement.ts).
    input: [input.optional(slot.ledger)],
});

const settle = sequence.prompt("settle", {
    title: "Settle the question (worker)",
    agent: agent.scout,
    text: input.prompt`
                    The interview on the plan ${port.plan} produced this round's probe: ${slot.probe}. Hold to the doctrine ${resource.doctrine}.
                    If its kind is fact, settle it yourself: explore the repository and environment with your tools and answer with concrete evidence — paths, commands, observed values. Never guess. A fact you cannot ground after genuinely looking becomes an owner decision, with a note of where you looked.
                    If its kind is decision, do NOT decide it. Sharpen it for the owner instead: the viable options, the tradeoff each carries, anything the repository already constrains — and keep the interrogator's recommendation attached.
                    Then update the interview ledger. If a ledger is attached to this frame, reproduce every existing entry verbatim and append exactly one entry for this round in the ledger's entry format; on round 1, start the ledger with this round's entry. Return the full updated ledger.`,
    output: slot.ledger,
    // This step reads its own output slot — sanctioned only in exactly this
    // shape: an OPTIONAL, non-interpolated input. A required or interpolated
    // self-input is rejected at build.
    input: [input.optional(slot.ledger)],
});

// An exhausted probe carries no question, so a settle frame would have
// nothing to do: gate it on `continue` rather than prompting the scout to
// no-op (pattern: plannotate's `implement-owner-gate` — `sequence.branch`
// with only a success arm).
const settleGate = sequence.branch("settle-gate", {
    check: condition.fieldEquals(slot.probe, "status", "continue"),
    success: [settle],
});

export default sequence.loop("interview-loop", {
    title: "One question per round, until the tree is exhausted",
    sequence: sequence.linear("interview-round", [probe, settleGate]),
    // Bounded on purpose, unlike plannotate's build loop: an interview has
    // diminishing returns, and the report states plainly when the budget
    // ended before the tree did — twelve questions is a long grilling.
    iterations: 12,
    until: condition.fieldEquals(slot.probe, "status", "exhausted"),
});
