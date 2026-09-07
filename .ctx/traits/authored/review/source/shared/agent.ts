import { reviewerRole } from "@ctx-traits/agents";

export const reviewer = reviewerRole(
  "reviewer",
  "Reviews an arbitrary git ref/range with no task board governing the run and writes both the typed verdict and the human review document.",
  "Standalone review role.",
);

// The pr variant's second seat. It never generates findings: it re-reads the
// range for each candidate and keeps only those it can restate as a failure
// path from the code it opened. Its own role id, so a runtime file can put a
// different model behind it than behind the reviewer.
export const verifier = reviewerRole(
  "verifier",
  "Re-reads the range for each candidate finding and keeps only those it can restate as a concrete failure path from the code; refutes or marks unverifiable the rest.",
  "Verification role.",
);
