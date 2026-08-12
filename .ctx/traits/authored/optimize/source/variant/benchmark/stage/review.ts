import { CODE_INTEGRITY_DOCTRINE, QUICK_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "../../../shared/agent.ts";
import { implementReceipt } from "../../../shared/data.ts";
import { draftSlot } from "../../../shared/data.ts";
import { reviewVerdictSlot, target } from "../data.ts";

export const reviewStage = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt(
    `Review the implemented candidate for {target} against the draft {draft}. Work summary: {implementReceipt}.
        A BLOCKER always includes a behavior break, a new smell, or an interface widened to make a caller compile. ${QUICK_VARIANT_DOCTRINE}
        This is the ONLY review pass for this round — an unapproved candidate is reverted, never repaired.
        Wherever the authority rule above names REVIEW_VERDICT_DOCTRINE, it means the composed baseline stated immediately below.
        ${CODE_INTEGRITY_DOCTRINE}`,
    {
      target,
      draft: draftSlot,
      implementReceipt,
    },
  ),
  output: reviewVerdictSlot,
});
