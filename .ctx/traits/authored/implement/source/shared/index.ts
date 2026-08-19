import * as cdk from "@ctx-traits/cdk";

export * as agent from "./agent.ts";
export * as data from "./data.ts";
export * as metadata from "./metadata.ts";
export * as resource from "./resource.ts";
export * as step from "./step/index.ts";

export const BEHAVIOR: cdk.Behavior = {
  tone: [cdk.tone.Direct, cdk.tone.Technical],
  method: cdk.method.EvidenceFirst,
  verbosity: cdk.verbosity.Brief,
};

export const INTENT: cdk.IntentSpec = {
  require: [
    cdk.intent.focus.Correctness,
    cdk.intent.require.Robustness,
    cdk.intent.require.Leanness,
    cdk.intent.require.ReuseOverReimplement,
    cdk.intent.require.ReviewBeforeFinal,
    cdk.intent.require.BoundedRefinement,
  ],
  avoid: [
    cdk.intent.avoid.OverEngineering,
    cdk.intent.avoid.Duplication,
    cdk.intent.avoid.ScopeCreep,
    cdk.intent.avoid.UnboundedLoop,
    cdk.intent.avoid.RubberStampReview,
  ],
};
