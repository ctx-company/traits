import { clerk, scribe, smart1Role, smart2Role, worker } from "#trait/shared/agent.ts";

export const smart1 = smart1Role("Strong model: drafts the implementation plan, and reviews the work in the refinement loop.");
export const smart2 = smart2Role(
  "Independent strong review model: reviews the implemented work each refinement pass, separately from smart-1.",
);
export { clerk, scribe, worker };
