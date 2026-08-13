import { reviewerRole } from "@ctx-traits/agents";

export const reviewer = reviewerRole(
  "reviewer",
  "Reviews an arbitrary git ref/range with no task board governing the run and writes both the typed verdict and the human review document.",
  "Standalone review role.",
);
