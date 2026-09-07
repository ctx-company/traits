import { CODE_INTEGRITY_DOCTRINE, reviewVerdictSchema } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";
import { input, port, schema, slot } from "@ctx-traits/cdk";

import { reviewer } from "../agent.ts";
import { range } from "../data.ts";
import { commitLog, diffStat } from "./evidence.ts";

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

// The pr variant's findings list. A finding is a defect the reviewer verified
// in the changed region; the list is what the code has, the verdict is what to
// do about it. Every convention lives in the field descriptions, not the prompt.
export const findingSchema = schema.object(
  "review-finding",
  {
    path: schema.field(schema.text(), {
      description: "Repo-relative path of the file the finding is in.",
    }),
    line: schema.field(schema.integer(), {
      description:
        "Line of that file, at the head of the range, where the defect is; the first line when it spans several. Absent only when the defect has no single location.",
      required: false,
    }),
    severity: schema.field(schema.enum(["low", "medium", "high", "critical"] as const), {
      description:
        "Impact if merged as is, never confidence: critical breaks or exposes something for every caller; high is a real defect on a main path; medium a real defect on a narrower path, or a correctness gap with a bounded blast radius; low a real defect with little practical impact.",
    }),
    category: schema.field(
      schema.enum(["bug", "security", "concurrency", "data", "api", "perf", "test_gap", "doc_defect"] as const),
      {
        description:
          "The kind of defect: bug (wrong behavior), security (a boundary crossed or crossable), concurrency (races, locking, ordering), data (loss, corruption, wrong persistence or keys), api (a contract or interface misused or broken), perf (avoidable cost the change introduces), test_gap (a changed behavior nothing exercises), doc_defect (a comment, message, string or document that is wrong).",
      },
    ),
    what: schema.field(schema.text(), {
      description:
        "The defect and its concrete consequence in one to three sentences that stand alone without the diff: what the code does, what it should do, and what goes wrong for whom.",
    }),
    evidence: schema.field(schema.text(), {
      description: "The offending lines quoted verbatim, and what was read or run to confirm the defect.",
    }),
  },
  { description: "One defect the reviewer verified in the changed region." },
);

export const findingsSchema = schema.object(
  "review-findings",
  {
    findings: schema.field(schema.list(findingSchema), {
      description:
        "Every defect verified in the changed region, one entry per underlying issue, listed whether or not it blocks the merge and whether or not the code was already wrong before the range: a defect of any severity may sit beside an approved verdict, and the verdict never removes an entry from this list. Not findings: merge-gate judgments (over-build, duplication, taste, scope), notes about the reviewing environment or what could not be run, and suspicions not confirmed in the code; those belong in the review document. Empty when the changed region has no verified defect.",
    }),
  },
  { description: "The typed findings list for the range: what the code has, independent of what to do about it." },
);

export const findings = slot({
  id: "findings",
  schema: findingsSchema,
  description: "The reviewer's typed findings for the range, written by the same reviewer step as the verdict and the document.",
});

// Bindings-form, not the tagged `input.prompt` template: `${CODE_INTEGRITY_DOCTRINE}`
// is a plain-string doctrine splice (JS interpolation at module-eval time, like
// implement's own DOCTRINE-splicing prompts), not a typed slot/port ref — the
// tagged template only accepts typed refs in its `${}` positions.
const reviewText = input.prompt(
  `Review the range {range}. Commit log: {commitLog}. File-level diff inventory (never the full patch): {diffStat}.
    Open the actual content yourself with your own tools — "git show", "git diff {range} -- <file>" — the inventory above is only an index of where the changes landed.
    No task board governs this run: leave "wall-id" the empty string, omit "remaining" and "owner-items", and set "escalation" to "needs-owner" only for authority questions the diff itself raises, never for ordinary code-review blockers.
    Edit nothing — you are read-only. Open files, do not write them.
    ${CODE_INTEGRITY_DOCTRINE}
    Write both the typed verdict and a rendered human review document: what the range does, the blockers you found (if any) and why each is one, and anything advisory.`,
  { range, commitLog, diffStat },
);

export const review = cdk.defineStep.prompt({
  agent: reviewer,
  input: reviewText,
  output: [reviewVerdict, reviewDocument],
});

const reviewPrText = input.prompt(
  `Review the range {range}. Commit log: {commitLog}. File-level diff inventory (never the full patch): {diffStat}.
    Open the actual content yourself with your own tools — "git show", "git diff {range} -- <file>" — the inventory above is only an index of where the changes landed.
    No task board governs this run: leave "wall-id" the empty string, omit "remaining" and "owner-items", and set "escalation" to "needs-owner" only for authority questions the diff itself raises, never for ordinary code-review blockers.
    Edit nothing — you are read-only. Open files, do not write them.
    ${CODE_INTEGRITY_DOCTRINE}
    Write the typed verdict, the typed findings list, and a rendered human review document: what the range does, each finding and why it carries its severity, which findings block the merge and why, and anything advisory.`,
  { range, commitLog, diffStat },
);

export const reviewPr = cdk.defineStep.prompt({
  agent: reviewer,
  input: reviewPrText,
  output: [reviewVerdict, findings, reviewDocument],
});

export const reviewVerdictReport = port.output.of("review-verdict-report", reviewVerdictSchema, {
  description: "The reviewer's typed verdict. Absent when the range held no changes.",
  optional: true,
  value: reviewVerdict,
});
export const reviewFindingsReport = port.output.of("review-findings-report", findingsSchema, {
  description: "The reviewer's typed findings list (pr variant). Absent when the range held no changes.",
  optional: true,
  value: findings,
});
export const reviewDocumentReport = port.output.text({
  id: "review-document-report",
  description:
    "The rendered human review document. Absent when the range held no changes.",
  optional: true,
  value: reviewDocument,
});
