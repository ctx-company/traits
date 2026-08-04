// Demo trait: the owner sits inside the build loop as a typed participant,
// via plannotator — annotating a plan the run refines against, and holding
// the outer gate the machine reviewer cannot open for it.
//
// A standalone package, not a member of the `implement` family and not a
// variant of it: the spine differs where it matters (two owner gates and an
// unbounded loop), so forking a variant here would produce a file that reads
// like the original and behaves nothing like it. Not a built-in either — it
// hard-depends on a third-party binary this project neither ships nor owns.
import { intent, method, procedure, sequence, tone, trait, verbosity } from "@ctx-traits/cdk";

import { slot } from "./data.ts";
import implementLoop from "./sequence/implement.ts";
import plan from "./sequence/plan.ts";
import { brief as summaryBrief, commit as summaryCommit, outputPorts } from "./sequence/summary.ts";

// First step, before any prompt frame: a missing binary fails here, not
// after a model call has already been spent drafting a plan it can never
// have annotated.
const preflightBinary = sequence.command("preflight-binary", {
    title: "Confirm plannotator is installed",
    argv: ["sh", "-c", "command -v plannotator"],
    output: slot.preflightOutput,
});

export default trait("plannotate", {
    version: "0.1.0",
    name: "Plannotate",
    summary:
        "Draft a plan, put the owner inside plannotator to annotate it, refine against the annotations, then loop worker and reviewer with no round limit until both the reviewer and the owner approve — writing a tracked brief and committing.",
    metadata: {
        tag: ["demo", "implementation", "review", "human-in-the-loop", "plannotator"],
    },
    behavior: {
        tone: [tone.Direct, tone.Technical],
        method: method.EvidenceFirst,
        verbosity: verbosity.Brief,
    },
    intent: {
        require: [intent.require.ReviewBeforeFinal, intent.require.RoleAttributedOutput],
        // Deliberately does NOT declare intent.avoid.UnboundedLoop — the
        // build loop having no iteration bound is this trait's thesis, not
        // an oversight: it ends when the work is right (reviewer AND owner
        // both approve), not on a round count picked to approximate "never".
        avoid: [intent.avoid.RubberStampReview, intent.avoid.ScopeCreep],
    },
    procedure: procedure({
        description:
            "Confirm plannotator is installed, draft a plan and put the owner inside plannotator to annotate it, refine the plan against the annotations, loop the worker and reviewer with no iteration bound until the reviewer and the owner both approve — the owner summoned only on a round the reviewer already approved — then write a tracked brief and commit.",
        output: [...outputPorts],
        sequence: [preflightBinary, ...plan, implementLoop, ...summaryBrief, ...summaryCommit],
    }),
});
