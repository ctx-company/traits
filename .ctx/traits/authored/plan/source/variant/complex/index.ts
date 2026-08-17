// plan-complex: plan-default's pipeline plus a genuine work -> review ->
// improve loop — an INDEPENDENT reviewer seat issues a typed verdict against
// the written board (format validity, coverage, sizing, dependency order),
// the composing seat applies every blocker, and the loop runs until approved
// or parks with a typed record. The MVP-planning variant.
import { defineVariant, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Complex", {
    name: "Plan (Complex)",
    summary:
      "Turn described work into board-ready TaskDocument TOML task files, then grind an independent-reviewer verdict loop over the written board until approved — coverage, sizing, dependencies, and format all blockable.",
    metadata: { tag: [...shared.metadata.tag, "review"] },
    description:
      "Turn described work into board-ready TaskDocument TOML task files, then grind an independent-reviewer verdict loop over the written board until approved — coverage, sizing, dependencies, and format all blockable.",
    procedureDescription:
      "Extract the source's work items and done criteria, ground the work in the codebase, split it into a typed slice plan with final keys, write each slice's TaskDocument TOML files in its own frame, then loop an independent typed-verdict review with composer-applied fixes until approved (parking with a typed record on exhaustion).",
  });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);

  const smart1 = shared.agent.smart1(
    "Strong model: extracts the source contract, grounds the work in the codebase, plans the slices, writes each slice's task files, and applies review fixes.",
    "Contract + grounding + planning + composition + revision role.",
  );
  const smart2 = shared.agent.smart2(
    "Independent strong model: reviews the written board each round, separately from smart-1.",
    "Independent review role.",
  );

  shared.stage.derive.nextKeyStep();
  shared.stage.derive.raisedDateStep();
  shared.stage.ingest.contract(smart1);
  shared.stage.refine.task(smart1);
  shared.stage.split.slices(smart1);
  shared.stage.writeSlices.tasks(smart1, 6);
  shared.stage.review.loop(smart2, smart1);

  return { writtenFiles: shared.data.writtenFiles, parkReport: shared.data.parkReportPort };
}
