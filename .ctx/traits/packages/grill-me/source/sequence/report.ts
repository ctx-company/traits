import { input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, resource, slot } from "../data.ts";

export default sequence.prompt("report", {
    title: "Write the grill report (smart-2)",
    agent: agent.synthesist,
    text: input.prompt`
                    The interview on the plan ${port.plan} has ended; the final probe is ${slot.probe}. Read its status FIRST: exhausted means the decision tree was walked to the end; continue means the round budget ran out with branches unvisited, and the report must say so plainly rather than implying completeness. Hold to the doctrine ${resource.doctrine}.
                    The settled rounds are in the interview ledger attached to this frame. If no ledger is attached, the very first probe found nothing to ask — write the report from the plan alone and say exactly that.
                    Write the grill report in three parts:
                    1. Shared understanding — the plan restated with every answered fact folded in, precise enough that a build started from it cannot go wrong on any settled point.
                    2. Decisions for the owner — every owner-decision entry from the ledger, one block each: the question, the options with their tradeoffs, and the recommended answer, phrased so the owner can settle it with a single reply.
                    3. Not settled — branches the budget never reached and any fact the scout could not ground. Omit this part only when there is truly nothing in it.
                    The interview changes nothing and builds nothing: this report is the run's entire product, and acting on the plan is the owner's move once the decision sheet is settled. Return only the report.`,
    output: slot.report,
    // `ledger` is optional and not interpolated: a first-round `exhausted`
    // probe skips the settle gate, and then no ledger was ever written.
    input: [input.optional(slot.ledger)],
});
