// review (pr): the default flow plus a typed findings list. Same deterministic
// evidence capture, same single read-only reviewer pass, same clean-tree
// assertion; the reviewer additionally returns every defect it verified in the
// changed region with severity and category, independent of the merge verdict.
// The list is what the code has; the verdict is what to do about it.
import { condition, defineVariant, flow, intent, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("pr", {
    name: "Review (PR)",
    summary:
      "Points a single reviewer at a git range and returns a typed verdict, a typed findings list (every verified defect with severity and category), and a rendered review document, with zero mutation of the reviewed tree.",
    metadata: { tag: ["first-party", "review", "read-only", "findings"] },
    description:
      "Deterministic evidence capture (diff --stat, log --oneline) -> single reviewer pass returning verdict, findings and document -> read-only assertion.",
  });
  useIntent({
    require: [intent.ReviewBeforeFinal, intent.Leanness],
    avoid: [intent.RubberStampReview, intent.ScopeCreep],
  });

  shared.step.evidence.captureDiffStat("Capture the diff inventory");
  shared.step.evidence.captureCommitLog("Capture the commit log");

  flow.when("Diff Non-Empty", condition.not(condition.equals(shared.step.evidence.diffStat, "")), () => {
    shared.step.review.reviewPr("Review the range");
    shared.step.assertClean.assert("Assert the reviewed tree stayed clean");
  });

  return {
    reviewVerdictReport: shared.step.review.reviewVerdictReport,
    reviewFindingsReport: shared.step.review.reviewFindingsReport,
    reviewDocumentReport: shared.step.review.reviewDocumentReport,
    treeStatusReport: shared.step.assertClean.treeStatusReport,
  };
}
