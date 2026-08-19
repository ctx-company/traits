import * as cdk from "@ctx-traits/cdk";

export * as agent from "./agent.ts";
export * as data from "./data.ts";
export * as metadata from "./metadata.ts";
export * as resource from "./resource.ts";
export * as step from "./step/index.ts";

export const BEHAVIOR: cdk.Behavior = {
  tone: [cdk.behavior.tone.Direct, cdk.behavior.tone.Technical],
  method: cdk.behavior.method.EvidenceFirst,
  verbosity: cdk.behavior.verbosity.Brief,
};

export const INTENT: cdk.Intent = {
  require: [
    cdk.intent.Correctness,
    cdk.intent.Robustness,
    cdk.intent.Leanness,
    cdk.intent.ReuseOverReimplement,
    cdk.intent.ReviewBeforeFinal,
    cdk.intent.BoundedRefinement,
  ],
  avoid: [
    cdk.intent.OverEngineering,
    cdk.intent.Duplication,
    cdk.intent.ScopeCreep,
    cdk.intent.UnboundedLoop,
    cdk.intent.RubberStampReview,
  ],
};
