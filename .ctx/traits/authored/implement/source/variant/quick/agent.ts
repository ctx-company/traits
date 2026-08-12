import { reviewerRole } from "@ctx-traits/agents";

import { scribe, worker } from "#trait/shared/agent.ts";

export const smart = reviewerRole(
  "smart-1",
  "Strong model: drafts the implementation plan, and is the sole reviewer in the refinement loop.",
  "Drafting and reviewer role.",
);
export { scribe, worker };
