import type { IntentSpec, ResourceHandle } from "@ctx-traits/cdk";
import { intent } from "@ctx-traits/cdk";

import { architectureDialect, smellCatalog } from "../resource.ts";

// The family-common intent core. Declared intents are the single home for
// these rules — prompts never restate them ("do not widen any interface",
// "run the gates", ...). Variants extend the core with their own
// doctrine-specific members as they convert onto the shared stages.
const coreRequire = [
    intent.require.ReviewBeforeFinal,
    intent.require.BoundedRefinement,
    intent.require.BehaviorPreservingDefault,
    intent.require.GatesGreenBeforeCommit,
];
const coreAvoid = [intent.avoid.RubberStampReview, intent.avoid.InterfaceWidening];

export const strictIntent: IntentSpec = {
    require: [...coreRequire, intent.require.VerbatimExecution],
    avoid: [...coreAvoid, intent.avoid.SilentDeviation],
};

export const resources: readonly ResourceHandle[] = [architectureDialect, smellCatalog];
