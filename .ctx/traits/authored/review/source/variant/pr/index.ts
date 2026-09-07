// review (pr): the default flow plus a verified findings list. Same deterministic
// evidence capture and clean-tree assertion; two read-only seats in between.
// The reviewer returns the verdict, the candidate findings and the document;
// the verifier re-reads the range for each candidate and writes the final
// findings list (confirmed candidates only) plus its per-candidate record.
// The list is what the code has; the verdict is what to do about it.
import { condition, defineVariant, flow, intent, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("pr", {
    name: "Review (PR)",
    summary:
      "Points a reviewer at a git range for a typed verdict, candidate findings and a review document, then a verifier that keeps only the findings it can restate as a failure path from the code; zero mutation of the reviewed tree.",
    metadata: { tag: ["first-party", "review", "read-only", "findings"] },
    description:
      "Deterministic evidence capture (diff --stat, log --oneline) -> reviewer pass (verdict, candidate findings, document) -> verifier pass (verification record, final findings) -> read-only assertion.",
  });
  useIntent({
    require: [intent.ReviewBeforeFinal, intent.Leanness],
    avoid: [intent.RubberStampReview, intent.ScopeCreep],
  });

  shared.step.evidence.captureDiffStat("Capture the diff inventory");
  shared.step.evidence.captureCommitLog("Capture the commit log");

  flow.when("Diff Non-Empty", condition.not(condition.equals(shared.step.evidence.diffStat, "")), () => {
    shared.step.review.reviewPr("Review the range");
    shared.step.review.verify("Verify the findings");
    shared.step.assertClean.assert("Assert the reviewed tree stayed clean");
  });

  return {
    reviewVerdictReport: shared.step.review.reviewVerdictReport,
    reviewFindingsReport: shared.step.review.reviewFindingsReport,
    reviewVerificationReport: shared.step.review.reviewVerificationReport,
    reviewDocumentReport: shared.step.review.reviewDocumentReport,
    treeStatusReport: shared.step.assertClean.treeStatusReport,
  };
}
