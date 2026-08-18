import * as cdk from "@ctx-traits/cdk";

export * as agent from "./agent.ts";
export * as data from "./data.ts";
export * as metadata from "./metadata.ts";
export * as resource from "./resource.ts";
export * as stage from "./stage/index.ts";

export const BEHAVIOR: cdk.Behavior = {
  tone: [cdk.tone.Direct, cdk.tone.Technical],
  method: cdk.method.EvidenceFirst,
  verbosity: cdk.verbosity.Brief,
};

export const INTENT: cdk.IntentSpec = {
  require: [
    cdk.intent.require.AnnotationFidelity,
    cdk.intent.require.BehaviorPreservingDefault,
    cdk.intent.require.BoundedRefinement,
    cdk.intent.require.GatesGreenBeforeCommit,
    cdk.intent.require.PreserveScope,
    cdk.intent.require.ReviewBeforeFinal,
    cdk.intent.require.VerbatimExecution,
  ],
  avoid: [
    cdk.intent.avoid.InterfaceWidening,
    cdk.intent.avoid.OverEngineering,
    cdk.intent.avoid.RubberStampReview,
    cdk.intent.avoid.ScopeCreep,
    cdk.intent.avoid.SilentDeviation,
  ],
};
