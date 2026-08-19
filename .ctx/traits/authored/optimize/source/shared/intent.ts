import type { Intent } from "@ctx-traits/cdk";
import * as cdk from "@ctx-traits/cdk";

export const benchmark = {
  require: [cdk.intent.ReviewBeforeFinal, cdk.intent.Correctness],
  avoid: [cdk.intent.RubberStampReview],
} as Intent;

export const experiment = {
  require: [cdk.intent.Correctness, cdk.intent.GatesGreenBeforeCommit],
  avoid: [cdk.intent.RubberStampReview],
} as Intent;
