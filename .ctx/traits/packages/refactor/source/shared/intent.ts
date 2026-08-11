import type { IntentSpec } from "@ctx-traits/cdk";
import * as cdk from "@ctx-traits/cdk";

export const intent: IntentSpec = {
    require: [
        cdk.intent.require.BehaviorPreservingDefault,
        cdk.intent.require.BoundedRefinement,
        cdk.intent.require.GatesGreenBeforeCommit,
        cdk.intent.require.ReviewBeforeFinal,
        cdk.intent.require.VerbatimExecution,
    ],
    avoid: [
        cdk.intent.avoid.InterfaceWidening,
        cdk.intent.avoid.RubberStampReview,
        cdk.intent.avoid.SilentDeviation,
    ],
};
