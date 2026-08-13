import { reviewVerdictSchema } from "@ctx-traits/agents";
import { schema } from "@ctx-traits/cdk";

export const reviewFindingSchema = schema.object(
  "review-finding",
  {
    finding: schema.field(schema.text(), {
      description: "What was checked and what was found, in the reviewer's own terms.",
    }),
    file: schema.field(schema.text(), { description: "Repo-relative path this finding cites." }),
    line: schema.field(schema.integer(), { description: "Line number in that file this finding cites." }),
  },
  {
    description:
      "One review-rubric finding: a concrete, source-anchored observation citing a repo-relative file and line.",
  },
);

// A bare `schema.list(reviewFindingSchema)` accepts `[]`, which would let a
// dimension go unchecked despite being `required` — `required` only enforces
// field presence, not list non-emptiness. Splitting each dimension into one
// mandatory `first` finding plus an optional `rest` list makes at least one
// source-anchored finding structurally unavoidable.
export function dimensionFindingsSchema(id: string, dimension: string) {
  return schema.object(
    id,
    {
      first: schema.field(reviewFindingSchema, { description: `The mandatory first ${dimension} finding.` }),
      rest: schema.field(schema.list(reviewFindingSchema), {
        description: `Any further ${dimension} findings beyond the mandatory first one.`,
      }),
    },
    { description: `${dimension}-dimension findings, with at least one finding required.` },
  );
}

// P334: the low end of the contract's "2-3" — a blocker still open after two
// consecutive raisings has already had one root-fix attempt fail its own
// done-when, which is the exact signal the recurrence breaker exists to
// catch.
export const RECURRENCE_BREAKER_ROUNDS = 2;

export const verdictSchema = schema.object(
  "review-verdict-default",
  schema.extend(reviewVerdictSchema, {
    scope: schema.field(dimensionFindingsSchema("scope-findings", "scope"), {
      description:
        "Scope-dimension findings: does the diff implement exactly what the task contract asks, source-anchored.",
    }),
    correctness: schema.field(dimensionFindingsSchema("correctness-findings", "correctness"), {
      description: "Correctness-dimension findings: is the implemented behavior actually right, source-anchored.",
    }),
    "gates-ran": schema.field(dimensionFindingsSchema("gates-ran-findings", "gates-ran"), {
      description:
        "Gates-ran-dimension findings: which of this task's Done-when gates actually ran, and their result, source-anchored.",
    }),
    "recurrence-rounds": schema.field(schema.integer(), {
      description:
        "The number of consecutive rounds the longest-standing still-open blocker has had exactly the same still-open step statuses with zero delta from the prior verdict: 1 for a first-raised blocker, 0 when blockers is empty. Reset it to 1 when any carried blocker step changes status or the blocker clears; never infer it from prose.",
    }),
    // DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
    // "stall-question": schema.field(schema.text(), {
    //     description:
    //         `Empty while recurrence-rounds is below ${RECURRENCE_BREAKER_ROUNDS}. At or above that threshold, one concrete, owner-answerable question that names the still-open blocker and the decision or access needed to clear it.`,
    // }),
  }),
);
