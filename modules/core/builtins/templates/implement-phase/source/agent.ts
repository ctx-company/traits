import { agent as cdkAgent } from "@ctx-traits/cdk";

// Three abstract roles. A multi-step trait typically uses one agent per
// step so each step's harness/model configuration can differ; a single
// agent reused across steps is equally valid when the roles do not need to
// diverge.
const planner = cdkAgent("planner", {
  description: "Drafts a concrete implementation approach for the stated task.",
  summary: "Implementation planner.",
});
const worker = cdkAgent("worker", {
  description: "Implements the draft, then reports what changed.",
  summary: "Implementer.",
});
const reviewer = cdkAgent("reviewer", {
  description: "Reviews the implemented work against the draft and reports a verdict.",
  summary: "Implementation reviewer.",
});

export const agent = { planner, worker, reviewer };
