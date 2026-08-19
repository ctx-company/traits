import { CODE_INTEGRITY_DOCTRINE, reviewVerdictSchema } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";
import { input, port, slot } from "@ctx-traits/cdk";

import { reviewer } from "../agent.ts";
import { range } from "../data.ts";
import { commitLog, diffStat } from "./evidence.ts";
import { rangeResolved } from "./resolve-range.ts";

export const reviewVerdict = slot({
  id: "review-verdict",
  schema: reviewVerdictSchema,
  description:
    "Reviewer's typed verdict on the range. No task board governs this run: wall-id is the empty string, remaining/owner-items are absent, and escalation is needs-owner only for authority questions the diff itself raises.",
});
export const reviewDocument = slot.text({
  id: "review-document",
  description: "Rendered human review document for the range, written by the same reviewer step.",
});

// Bindings-form, not the tagged `input.prompt` template: `${CODE_INTEGRITY_DOCTRINE}`
// is a plain-string doctrine splice (JS interpolation at module-eval time, like
// implement's own DOCTRINE-splicing prompts), not a typed slot/port ref — the
// tagged template only accepts typed refs in its `${}` positions.
const reviewText = input.prompt(
  `Review the range {rangeResolved} (from the range/ref you were given: {range}). Commit log: {commitLog}. File-level diff inventory (never the full patch): {diffStat}.
    Open the actual content yourself with your own tools — "git show", "git diff {rangeResolved} -- <file>" — the inventory above is only an index of where the changes landed.
    No task board governs this run: leave "wall-id" the empty string, omit "remaining" and "owner-items", and set "escalation" to "needs-owner" only for authority questions the diff itself raises, never for ordinary code-review blockers.
    Edit nothing — you are read-only. Open files, do not write them.
    ${CODE_INTEGRITY_DOCTRINE}
    Write both the typed verdict and a rendered human review document: what the range does, the blockers you found (if any) and why each is one, and anything advisory.`,
  { range, rangeResolved, commitLog, diffStat },
);

export const review = cdk.step({
  agent: reviewer,
  input: reviewText,
  output: [reviewVerdict, reviewDocument],
});

export const reviewVerdictReport = port.output.of(reviewVerdictSchema, {
  id: "review-verdict-report",
  description: "The reviewer's typed verdict. Absent when preflight parked on an unresolvable range or an empty diff.",
  optional: true,
  value: reviewVerdict,
});
export const reviewDocumentReport = port.output.text({
  id: "review-document-report",
  description:
    "The rendered human review document. Absent when preflight parked on an unresolvable range or an empty diff.",
  optional: true,
  value: reviewDocument,
});
