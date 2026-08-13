import type { IntentSpec } from "@ctx-traits/cdk";
import * as cdk from "@ctx-traits/cdk";

export const benchmark = {
  require: [cdk.intent.require.ReviewBeforeFinal, cdk.intent.focus.Correctness],
  avoid: [cdk.intent.avoid.RubberStampReview],
} as IntentSpec;

export const experiment = {
  require: [cdk.intent.focus.Correctness, cdk.intent.require.GatesGreenBeforeCommit],
  avoid: [cdk.intent.avoid.RubberStampReview],
} as IntentSpec;
