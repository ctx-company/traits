// Demo trait: the annotation-driven implementation procedure.
//
// NOT a member of the implement family and deliberately not a variant of it:
// no `variant` map, no family metadata, no import from
// implement-default/source/shared.ts. It exists to demonstrate one shape the
// family does not have — the assignment arriving as typed annotations from an
// external tool rather than from a plan file or a caller-supplied port — so it
// owns its own small procedure end to end, and ships through the gate rather
// than stopping at a commit.
import { intent, method, procedure, tone, trait, verbosity } from "@ctx-traits/cdk";

import { port } from "./data.ts";
import annotation from "./sequence/annotation.ts";
import commit from "./sequence/commit.ts";
import draft from "./sequence/draft.ts";
import implementation from "./sequence/implementation.ts";
import walkthrough from "./sequence/walkthrough.ts";

export default trait("implement-guided", {
    version: "0.1.0",
    name: "Implement Guided",
    summary:
        "Take the assignment as annotations from the annotation tool, draft it, implement it under bounded review, explain the approved diff hunk by hunk, then commit and push through the gate.",
    metadata: {
        tag: ["demo", "implementation", "review", "human-in-the-loop"],
    },
    behavior: {
        tone: [tone.Direct, tone.Technical],
        method: method.EvidenceFirst,
        verbosity: verbosity.Brief,
    },
    intent: {
        require: [intent.require.ReviewBeforeFinal, intent.require.BoundedRefinement, intent.require.RoleAttributedOutput],
        avoid: [intent.avoid.UnboundedLoop, intent.avoid.RubberStampReview, intent.avoid.ScopeCreep],
    },
    procedure: procedure({
        description:
            "Collect the assignment as annotations, draft it, implement it under bounded review, write a markdown walkthrough of the approved diff, then commit and push through the gate.",
        output: [port.changeWalkthrough, port.commitReport, port.pushReport],
        sequence: [annotation, draft, implementation, ...walkthrough, ...commit],
    }),
});
