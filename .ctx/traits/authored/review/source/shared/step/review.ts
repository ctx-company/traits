import { CODE_INTEGRITY_DOCTRINE, reviewVerdictSchema } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";
import { input, port, schema, slot } from "@ctx-traits/cdk";

import { reviewer, verifier } from "../agent.ts";
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

// The pr variant's findings. A finding is a defect verified in the changed
// region; the list is what the code has, the verdict is what to do about it.
// Two seats touch it: the reviewer generates candidates, the verifier keeps
// the ones it can restate as a failure path. Every convention lives in the
// field descriptions, not the prompts.
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
      description:
        "The failure path that makes the defect observable: the triggering input or state, the call path from an entry point or caller that reaches the lines, and the wrong outcome, with the offending lines quoted verbatim. A description of what was read is not evidence; a defect whose failure path cannot be stated is a suspicion and belongs in the review document, not here.",
    }),
  },
  { description: "One defect verified in the changed region, with the failure path that makes it observable." },
);

export const findingsSchema = schema.object(
  "review-findings",
  {
    findings: schema.field(schema.list(findingSchema), {
      description:
        "Every defect verified in the changed region, one entry per underlying issue, listed whether or not it blocks the merge and whether or not the code was already wrong before the range: a defect of any severity may sit beside an approved verdict, and the verdict never removes an entry from this list. Not findings: merge-gate judgments (over-build, duplication, taste, scope), notes about the reviewing environment or what could not be run, and suspicions not confirmed in the code; those belong in the review document. Empty when the changed region has no verified defect.",
    }),
  },
  { description: "A typed findings list for the range: what the code has, independent of what to do about it." },
);

export const candidateFindings = slot({
  id: "candidate-findings",
  schema: findingsSchema,
  description:
    "The reviewer's candidate findings, written by the same step as the verdict and the document. Candidates, not the result: the verifier decides which of them are findings.",
});

export const findings = slot({
  id: "findings",
  schema: findingsSchema,
  description:
    "The final findings for the range, written by the verifier: the confirmed candidates verbatim, in candidate order, each with its evidence replaced by the failure path the verifier restated from the code. Nothing the verifier did not confirm appears here.",
});

export const verificationSchema = schema.object(
  "review-verification",
  {
    candidates: schema.field(
      schema.list(
        schema.object(
          "review-verification-item",
          {
            index: schema.field(schema.integer(), {
              description: "Position of the candidate in the candidate findings list, counting from zero.",
            }),
            disposition: schema.field(schema.enum(["confirmed", "refuted", "unverifiable"] as const), {
              description:
                "confirmed only when the verifier restated the failure path from code it opened itself: the triggering input or state, a caller, entry point or test that actually reaches the lines, and the wrong outcome. refuted when the code shows the failure cannot occur, naming the line that prevents it or the caller that never passes the triggering input. unverifiable when deciding would need a runtime, data or service the verifier cannot see; never used as a soft confirm.",
            }),
            "failure-path": schema.field(schema.text(), {
              description:
                "The failure path as restated from the code, with the lines quoted; required for confirmed, and for refuted the line or fact that blocks it. Empty only for unverifiable.",
            }),
            note: schema.field(schema.text(), {
              description: "One sentence for the human: why this disposition, in the verifier's own words.",
              required: false,
            }),
          },
          { description: "The verifier's decision on one candidate finding." },
        ),
      ),
      { description: "One decision per candidate, every candidate present exactly once, in candidate order." },
    ),
  },
  { description: "The verifier's record over the candidate findings: what was confirmed, refuted or left unverifiable, and why." },
);

export const verification = slot({
  id: "verification",
  schema: verificationSchema,
  description: "The verifier's per-candidate record, written together with the final findings.",
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
    Write the typed verdict, the candidate findings, and a rendered human review document: what the range does, each candidate and why it carries its severity, which of them block the merge and why, and anything advisory. A second seat verifies the candidates afterwards; list what you found, it decides what stands.`,
  { range, commitLog, diffStat },
);

export const reviewPr = cdk.defineStep.prompt({
  agent: reviewer,
  input: reviewPrText,
  output: [reviewVerdict, candidateFindings, reviewDocument],
});

export const verify = cdk.defineStep.prompt({
  agent: verifier,
  input: cdk.input.prompt`
    Verify the candidate findings ${candidateFindings} against the range ${range}.
    Open the code yourself — "git show", "git diff ${range} -- <file>", grep for callers and tests — the candidates' own evidence is a claim to check, never something to copy.
    Edit nothing — you are read-only.
    Return the verification record and the final findings list.
  `,
  output: [verification, findings],
});

export const reviewVerdictReport = port.output.of("review-verdict-report", reviewVerdictSchema, {
  description: "The reviewer's typed verdict. Absent when the range held no changes.",
  optional: true,
  value: reviewVerdict,
});
export const reviewFindingsReport = port.output.of("review-findings-report", findingsSchema, {
  description: "The verified findings list (pr variant). Absent when the range held no changes.",
  optional: true,
  value: findings,
});
export const reviewVerificationReport = port.output.of("review-verification-report", verificationSchema, {
  description: "The verifier's per-candidate record (pr variant). Absent when the range held no changes.",
  optional: true,
  value: verification,
});
export const reviewDocumentReport = port.output.text({
  id: "review-document-report",
  description:
    "The rendered human review document. Absent when the range held no changes.",
  optional: true,
  value: reviewDocument,
});
