import type { DeclaredSignalWithFields, SchemaHandle } from "@ctx-traits/cdk";
import { schema, signal } from "@ctx-traits/cdk";

/** The kit's built-in owner-escalation payload: why only the owner can settle this, and the one question that unblocks the run. */
export type NeedsOwnerValue = {
  readonly reason: string;
  readonly question: string;
};

/** Payload schema for {@link needsOwnerSignal}. Own module rather than `schema.ts`: this shape exists only as this signal's payload. */
export const needsOwnerSchema: SchemaHandle<NeedsOwnerValue> = schema.object(
  "needs-owner-payload",
  {
    reason: schema.field(schema.text(), {
      description:
        "Why only the owner can settle this: the contradiction, the missing authority, or the trade-off the run has no standing to decide. Grounded in what you verified, not a restatement of the question.",
    }),
    question: schema.field(schema.text(), {
      description:
        "The single question to put to the owner, phrased so one short answer unblocks the run. Name the options you see and what you would do absent an answer.",
    }),
  },
  {
    description: "Why only the owner can settle this, and the one question that unblocks the run.",
  },
);

/**
 * The kit's built-in owner-summons signal (0253.5): the active counterpart to a reviewer verdict's
 * `escalation: needs-owner` field. A trait that wires a `step.ask` guarded on this signal parks on
 * a summons the owner answers inside the run; a trait that never wires it keeps today's passive
 * park-for-triage meaning (see `REVIEW_VERDICT_DOCTRINE`'s Escalation paragraph). Declaring this
 * constant costs an importing trait nothing until it is guarded on — `signal(...)` only mints a
 * `[[signal]]`/`[[schema]]` entry into a trait's own canonical when something in that trait's
 * procedure actually references the handle.
 */
export const needsOwnerSignal: DeclaredSignalWithFields<NeedsOwnerValue> = signal({
  id: "needs-owner",
  description: "The run cannot proceed without an owner decision; parks on a summons the owner answers inside the run.",
  schema: needsOwnerSchema,
});
