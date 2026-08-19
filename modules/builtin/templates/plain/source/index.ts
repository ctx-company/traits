// The smallest trait that builds.
//
// No steps and no agents, so nothing runs yet — what a trait declares here is
// how work should be done, which any trait that does have steps then inherits.
// Add a port for what comes in, a slot for what a step produces, and an agent
// step to produce it.
import * as cdk from "@ctx-traits/cdk";

export default function () {
  cdk.defineTrait("Plain", {
    version: "0.1.0",
    description: "Describe what this trait is for.",
    metadata: { tag: ["template"] },
  });

  // Rendered into every step. Selecting an item is the instruction.
  cdk.useBehavior({
    tone: [cdk.behavior.tone.Plain, cdk.behavior.tone.Direct],
    method: [cdk.behavior.method.EvidenceFirst],
  });
  cdk.useIntent({
    focus: [cdk.intent.Correctness],
    avoid: [cdk.intent.ScopeCreep],
  });
}
