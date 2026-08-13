import { resource } from "@ctx-traits/cdk";

import { declareTaskBoard } from "#trait/shared/resource.ts";

export const taskBoard = declareTaskBoard();

// implement-phase-only: the richer review rubric (P276) is this package's
// own on-demand resource, threaded optionally into the shared review
// builders — sibling implement variants that never pass one stay
// byte-equivalent.
export const reviewRubric = resource({
  id: "review-rubric",
  path: "resources/review-rubric.md",
  hint: "The three required review dimensions (scope, correctness, gates-ran), their evidence expectations, and the blocker/status relationship; agents read this file with their own tools.",
  trigger: "on-demand",
});
