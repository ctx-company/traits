import {
  blockerSchema,
  clerkRole,
  LEFTOVER_DOCTRINE,
  leftoverSchema,
  reviewerRole,
  REVIEW_VERDICT_DOCTRINE,
  reviewVerdictSchema,
  scribeRole,
  workerRole,
} from "@ctx-traits/agents";
import { prompt, slot, trait } from "@ctx-traits/cdk";

const workerAgent = workerRole("worker", "Implements a draft or agreed design and applies reviewer fixes.");
const reviewerAgent = reviewerRole("reviewer", "Drafts and/or reviews the work in a bounded refinement loop.");
const scribeAgent = scribeRole("scribe", "Writes the commit message for a completed run");
const clerkAgent = clerkRole("clerk", "Extracts and distills context so later steps never re-read source files.");

const phaseBrief = slot.text({
  id: "phase-brief",
  description: "The scope contract the doctrine's judgment clause binds to — a consumer's phase or design contract.",
});
const productBrief = slot.text({
  id: "product-brief",
  description:
    "The house-rules digest the doctrine's judgment clause binds to — a consumer's product-contract summary.",
});
const reviewVerdictDoctrine = prompt({
  id: "review-verdict-doctrine",
  description:
    "Shared blocker-reporting and escalation doctrine for a typed multi-reviewer refinement loop: how to judge severity, report a blocker, and record owner-triage escalation.",
  text: prompt.template(REVIEW_VERDICT_DOCTRINE, { phaseBrief, productBrief }),
});
const leftoverDoctrine = prompt({
  id: "leftover-doctrine",
  description:
    "Shared leftover-review doctrine: the two questions a reviewer applies to every worker-proposed leftover before it survives into slot:leftovers.",
  text: prompt.template(LEFTOVER_DOCTRINE, {}),
});

export default trait("agents", {
  version: "0.2.0",
  name: "Agents",
  summary:
    "Shared reviewed-multi-agent doctrine: the role declarations, review/escalation prompt doctrine, and typed review-verdict schema a bounded refinement loop reports findings against, for dependents that declare it instead of pasting the roles, doctrine, or schema into their own trait source.",
  metadata: {
    tag: ["first-party", "knowledge", "agents"],
  },
  agent: [workerAgent, reviewerAgent, scribeAgent, clerkAgent],
  prompt: [reviewVerdictDoctrine, leftoverDoctrine],
  schema: [blockerSchema, reviewVerdictSchema, leftoverSchema],
});
