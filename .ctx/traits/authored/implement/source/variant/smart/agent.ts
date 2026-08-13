import { clerk, scribe, smart1Role, smart2Role, worker } from "#trait/shared/agent.ts";

export const smart1 = smart1Role(
  "Strong model with research tools: researches prior art before drafting, and reviews the work in the build loop.",
);
export const smart2 = smart2Role(
  "Independent strong review model: reviews the built work each build round, separately from smart-1.",
);
export { clerk, scribe, worker };
