// review (default): deterministic evidence capture (stat inventory + commit
// log, never patch bodies) -> single reviewer pass -> read-only assertion. No
// refinement loop, no worker, no commit tail: there is nothing to fix here,
// only to judge. The range goes to git as written — git rejects one it cannot
// resolve — and an empty diff lets the flow.when close over nothing further,
// the same maybe-commit idiom every other family uses.
import { condition, defineVariant, flow, intent, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("default", {
    name: "Review (Default)",
    summary:
      "Points a single reviewer at a git range and returns a typed verdict plus a rendered review document, with zero mutation of the reviewed tree.",
    metadata: { tag: ["first-party", "review", "read-only"] },
    description:
      "Deterministic evidence capture (diff --stat, log --oneline) -> single reviewer pass -> read-only assertion.",
  });
  useIntent({
    require: [intent.ReviewBeforeFinal, intent.Leanness],
    avoid: [intent.RubberStampReview, intent.ScopeCreep],
  });

  shared.step.evidence.captureDiffStat("Capture the diff inventory");
  shared.step.evidence.captureCommitLog("Capture the commit log");

  flow.when("Diff Non-Empty", condition.not(condition.equals(shared.step.evidence.diffStat, "")), () => {
    shared.step.review.review("Review the range");
    shared.step.assertClean.assert("Assert the reviewed tree stayed clean");
  });

  return {
    reviewVerdictReport: shared.step.review.reviewVerdictReport,
    reviewDocumentReport: shared.step.review.reviewDocumentReport,
    treeStatusReport: shared.step.assertClean.treeStatusReport,
  };
}
