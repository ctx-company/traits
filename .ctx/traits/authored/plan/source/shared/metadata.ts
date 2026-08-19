import * as cdk from "@ctx-traits/cdk";

export const tag = ["task", "plan", "bootstrap", "planning"];
export const behavior: cdk.Behavior = {
  tone: [cdk.behavior.tone.Direct, cdk.behavior.tone.Technical],
  method: cdk.behavior.method.EvidenceFirst,
  verbosity: cdk.behavior.verbosity.Brief,
};
