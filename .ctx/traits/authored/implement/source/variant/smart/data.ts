import { planAmendmentSchema, reviewerVerdict, reviewVerdictSchema } from "@ctx-traits/agents";
import { port, schema, slot } from "@ctx-traits/cdk";

export const planningVerdict = slot({
  id: "planning-verdict",
  schema: reviewerVerdict,
  description: 'Reviewer verdict for the "planning" produce-review round.',
});

export const researchNotes = slot.text({
  id: "research-notes",
  description:
    "Prior art and precedent gathered before the draft: how comparable tasks or the house's own history handled this problem, and any external guidance worth grounding the draft in.",
  hint: "Better inputs, not more rounds — this step runs once, before the draft, and is never repeated.",
});

export const verdictSchema = schema.object(
  "review-verdict-smart",
  schema.extend(reviewVerdictSchema, {
    amendments: schema.field(schema.list(planAmendmentSchema), {
      required: false,
      description:
        "Typed amendments to the agreed draft under smart authority, present when the draft and the observed work genuinely contradicted. Each becomes the binding contract going forward; an unrecorded departure is still a plan-fidelity gap, not an amendment.",
    }),
  }),
);
export const verdict1 = slot({
  id: "review-verdict-1",
  schema: verdictSchema,
  description: "smart-1's refinement verdict for the current work state.",
});
export const verdict2 = slot({
  id: "review-verdict-2",
  schema: verdictSchema,
  description: "smart-2's refinement verdict for the current work state.",
});

export const buildingSealed = slot.boolean("building-sealed");

/**
 * This variant's own park-report list, declared locally (not the shared
 * `#trait/shared/data.ts` `parkReport`/`parkReportPort`) because its
 * verdict schema is EXTENDED with `amendments` — a `project` step can only
 * copy `verdict2`'s whole accepted value onto a destination whose declared
 * schema matches it exactly, so the park-report list must share this
 * variant's own extended `verdictSchema`, not the family base.
 */
export const parkReport = slot({
  id: "park-report",
  schema: schema.list(verdictSchema),
  description:
    "This round's typed park record (P414): empty when the round's verdict is approved, exactly one entry — the round's own verdict, copied unchanged — when it is revise. Written each round by a deterministic project step, never model-authored, so it can never disagree with the verdict it comes from.",
});
export const parkReportPort = port.output.of(schema.list(verdictSchema), {
  id: "park-report",
  title: "Park Report",
  description:
    "Typed park record for an unapproved run (P414): the wall citation (if any), the exact blockers, and escalation state. Present in the run's persisted output-port evidence only when the build loop exhausted without approval — the run parks and no commit is created. A dispatch-time preflight refuses a sibling task that explicitly cites the same wall-id while this stands unforced.",
  optional: true,
  value: parkReport,
  format: ["structured", "table"],
});
