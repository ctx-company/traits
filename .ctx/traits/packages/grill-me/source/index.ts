// Ported from Matt Pocock's `grill-me` / `grilling` skills
// (github.com/mattpocock/skills, MIT): a relentless interview that
// stress-tests a plan one question at a time, in decision-tree dependency
// order, a recommended answer attached to every question. The port keeps the
// skill's fact/decision split but runs headless: facts are settled by a
// tool-equipped scout instead of blocking on a human, and the decisions —
// which the skill insists are the owner's alone — accumulate into a decision
// sheet the owner settles in one sitting. Nothing is acted on: the grill
// report is the run's entire product, the skill's "do not act until I
// confirm" made structural.
import { intent, method, procedure, tone, trait, verbosity } from "@ctx-traits/cdk";

import { port } from "./data.ts";
import interviewLoop from "./sequence/interview.ts";
import report from "./sequence/report.ts";

export default trait("grill-me", {
    version: "0.1.0",
    name: "Grill Me",
    summary:
        "Stress-test a plan with a relentless one-question-at-a-time interview: facts settled from the repository by a scout, genuine decisions sharpened into an owner decision sheet, and a grill report as the only product — nothing is built.",
    metadata: {
        tag: ["first-party", "planning", "interview", "multi-agent"],
    },
    behavior: {
        tone: [tone.Curious, tone.Critical, tone.Direct],
        method: [method.Socratic, method.EvidenceFirst],
        verbosity: verbosity.Brief,
    },
    intent: {
        require: [intent.require.StateAssumptions, intent.require.BoundedRefinement, intent.require.RoleAttributedOutput],
        avoid: [intent.avoid.SpeculativeClaim, intent.avoid.UnboundedLoop, intent.avoid.ScopeCreep],
    },
    // Trait-level, not `procedure({ output })`: the procedure-level form is
    // silently dropped from the canonical (no `[[port]]`, no
    // `[procedure] output` — plannotate's generated/index.toml shows the
    // same loss), while this form is the one guarded-change proves out.
    port: port.grillReport,
    procedure: procedure({
        description:
            "Interview the plan one question per round in decision-tree dependency order — the interrogator asks with a recommended answer, the scout settles facts from the repository and queues genuine decisions for the owner, the ledger accumulates every round — then write the grill report: shared understanding, the owner decision sheet, and what was not settled.",
        sequence: [interviewLoop, report],
    }),
});
