// Teaching template: implement-phase
//
// Scaffolded by `ctx traits new <name> --from implement-phase`. Teaches the
// same shape as this repository's own dogfood `.ctx/traits/implement-phase/`
// procedure — draft an approach, implement it, review the result — but is
// deliberately portable rather than a verbatim copy: the live procedure
// imports role helpers and shared prompt doctrine from a repo-local
// `@ctx-traits/agents` package that is not published for external use, so
// this template inlines equivalent `agent(...)` roles instead of that
// import. Copying the import verbatim would make a package scaffolded
// outside this repository fail its first build; this version builds and
// runs anywhere `@ctx-traits/cdk` is installed.
//
// This isn't a paper design: this project's own dogfood draft/implement/
// review procedure has produced real merged work. The authoring contract
// for this template asked specifically for P197 run evidence; no P197
// commit, ledger entry, or transcript exists in this repository's tree or
// `git log --all` history (verified, not merely unsearched), so that
// citation cannot be produced — this is a contract gap for the owner to
// resolve, not something to paper over by swapping in a different phase
// number. Commit fadf01efa692a ("P269: `ctx traits doctor` — skills-folder
// x-ray as the adoption front door"), reachable in this repo's own
// `git log --oneline` history rather than in gitignored, ephemeral run
// transcripts, is offered only as a separate, general illustration that
// this project's own draft/implement/review shape has produced merged
// work — it is not a substitute for the required P197 citation.
//
// The live procedure is also considerably larger: an extraction pass over a
// plan file, a bounded doubly-reviewed refinement loop (`sequence.loop`),
// and command steps that stage and commit the result. This template keeps
// the teaching shape — plan, implement, review — as three linear
// `sequence.prompt` steps, each with its own agent, chained by passing one
// step's output slot into the next step's input. Reach for `sequence.loop`
// and `sequence.command` (see the CDK's `sequence` export) once you need a
// bounded refinement loop or a real shell command in your own trait.
//
// P533: this is also the worked example of a multi-module template —
// `data.ts` for the ports/slots, `schema.ts` for the verdict shape,
// `agent.ts` for the three roles, and one `sequence/<concept>.ts` per step —
// so `index.ts` itself only imports and composes. See
// `packages/cdk/README.md`'s "Package layout" section for the full doctrine.

import { port as cdkPort, procedure, ref, trait } from "@ctx-traits/cdk";

import { port } from "./data.ts";
import draft from "./sequence/draft.ts";
import implement from "./sequence/implementation.ts";
import review, { verdict } from "./sequence/review.ts";

// The output port: bound directly to the review step's `output.of(...)`
// instruction-output, so the port, the auto-declared slot, and the schema
// all trace back to that one declaration instead of a separately
// hand-declared slot.
const output = cdkPort.output.of({
  id: "verdict",
  schema: ref.schema("implementation-verdict"),
  description: "The reviewer's final verdict on the implemented work.",
  value: verdict,
});

export default trait("implement-phase", {
  version: "0.1.0",
  name: "Implement Phase",
  description:
    "Drafts an implementation approach for a stated task, implements it, then reviews the result and reports a verdict.",
  metadata: { tag: ["template", "implementation", "review"] },
  procedure: procedure({
    description: "Plan, implement, and review one task end to end.",
    input: port.task,
    output,
    // Three chained prompt steps. Each step's `output` slot becomes the
    // next step's `input`, so the model sees exactly the prior step's
    // structured/text result, never the whole conversation.
    sequence: [draft, implement, review],
  }),
});
