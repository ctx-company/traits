import type {
  AgentHandle,
  GuardValue,
  OptionalSlotRead,
  PromptTemplate,
  SchemaHandle,
  SequenceFunction,
  SequenceHandle,
  SlotHandle,
  SlotWithFields,
} from "@ctx-traits/cdk";
import { condition, input, operation, prompt, schema, sequence, slot, withHiddenField } from "@ctx-traits/cdk";

/** The kit's minimal produce-review verdict value: `schema.decision()`'s stable `approved|revise` vocabulary plus the blockers that justify a `revise`. */
export type ReviewerVerdictValue = {
  readonly status: "approved" | "revise";
  readonly blockers: readonly string[];
};

/**
 * The kit's typed produce-review verdict: `status` built directly from
 * `schema.decision()` (P449's rider — the shared decision vocabulary is the
 * verdict spine, not a fourth hand-rolled enum) plus a plain blocker list.
 * Deliberately poorer than the family's own `reviewVerdictSchema` (blocker
 * ids, owner-items, escalation, ...) — that richer shape stays untouched
 * for P450; this is the kit's own, smaller contract.
 */
export const reviewerVerdict: SchemaHandle<ReviewerVerdictValue> = schema.object(
  "reviewer-verdict",
  {
    status: schema.field(schema.decision(), {
      description: "approved when no blocker remains; revise when at least one blocker remains.",
    }),
    blockers: schema.field(schema.list(schema.text()), {
      description: "The blocking defects that justify a revise verdict; empty when status is approved.",
    }),
  },
  {
    description: "Minimal produce-review verdict: decision status plus the blockers that justify a revise.",
  },
);

/** One caller-owned side of a `guardedProduction` round: the agent and the prompt text it runs. */
export type GuardedProductionRole = {
  readonly agent: AgentHandle;
  readonly text: PromptTemplate;
};

/**
 * A produce role plus caller-owned slots the produce step should read
 * hidden-until-produced (`input.optional`), the same round-1-hidden
 * mechanism the kit already applies to every review seat's own verdict —
 * e.g. the prior round's adjudicated leftovers or gate result, which a
 * produce-first round one (before any review has ever run) cannot require.
 * `text` must not interpolate these refs directly for the same reason it
 * must not interpolate the verdict: they are already attached as context
 * via `input:`, and a duplicate literal ref trips `cdk-duplicate-input`.
 */
export type GuardedProductionProduceRole = GuardedProductionRole & {
  readonly optionalInputs?: readonly SlotHandle[];
};

/**
 * One reviewer seat in a `guardedProduction` round. Extends the plain
 * `GuardedProductionRole` with three additive, caller-optional overrides:
 * a caller-owned verdict slot (instead of the kit inventing `<id>-verdict`),
 * a caller-owned verdict schema (instead of the kit's minimal
 * `reviewerVerdict`), and extra output slots the seat's review step also
 * writes each round (e.g. a leftovers list). All three default to the
 * original single-seat behavior when omitted.
 */
export type GuardedProductionReviewSeat = GuardedProductionRole & {
  /** Overrides the invented `<id>-verdict` slot with a caller-declared one. */
  readonly verdictSlot?: SlotHandle;
  /** Overrides the kit's minimal `reviewerVerdict` schema for an invented verdict slot. Ignored when `verdictSlot` is given (its own schema already governs). */
  readonly verdictSchema?: SchemaHandle;
  /** Extra slots this seat's review step also writes each round, alongside its verdict. */
  readonly extraOutputs?: readonly SlotHandle[];
  /**
   * Extra inputs this seat's review step reads, attached as frame context
   * rather than interpolated into its prompt. The point of the escape is a
   * loop-carried read — including the seat reading its OWN verdict from the
   * previous round, which is how an unresolved blocker survives into the next
   * round instead of being silently replaced. Pass `slot.optional()` for that
   * case: core exempts optional inputs from the producer-before-consumer
   * check, and (deliberately) refuses an optional ref that a prompt
   * interpolates, since an absent value would leave the token unresolved.
   */
  readonly extraInputs?: readonly (SlotHandle | OptionalSlotRead)[];
};

export type GuardedProductionOptions<Produces> = {
  readonly id: string;
  /** Caller-declared slot(s) the produce step writes its artifact into. */
  readonly produces: SlotHandle<Produces> | readonly SlotHandle[];
  /** Caller-owned producer role and prompt text; the kit synthesizes no prose. */
  readonly produce: GuardedProductionProduceRole;
  /** Caller-owned reviewer seat(s) and prompt text; the kit synthesizes no prose. `until` requires every seat's verdict approved. A bare seat behaves exactly as the original single-seat shape (same invented ids); an array of N seats numbers its steps/invented ids `-1..-N`. */
  readonly review: GuardedProductionReviewSeat | readonly GuardedProductionReviewSeat[];
  /** Caller-owned steps run between produce and review each round (e.g. a gate check, a scoped diff capture). Their output slots are caller-declared. */
  readonly evidence?: readonly SequenceHandle[];
  /** Caller-owned steps run after every seat's review this round and before the exhaustion check (e.g. a typed park-report derivation) — the seam `evidence` cannot express since it runs before review. */
  readonly afterReview?: readonly SequenceHandle[];
  /** An additional guard evaluated alongside every seat's verdict approval, e.g. a gate-green check. */
  readonly alsoRequire?: GuardValue;
  /** Caller-owned list of carried review items. When supplied, the loop only exits after no item has an empty `needs` field. */
  readonly carry?: SlotHandle;
  /** Optional 1-based minimum round for a non-empty carry list; requires `carry` and must be greater than 1 but less than `rounds`. */
  readonly minRounds?: number;
  readonly rounds: number;
  readonly onExhausted?: Parameters<SequenceFunction["loop"]>[1]["onExhausted"];
  /** Forwarded verbatim to `sequence.loop`'s own early-stop guard; undeclared for every caller that never passes it, so the built loop stays byte-identical there. */
  readonly stopIf?: GuardValue;
  /** Forwarded verbatim to `sequence.loop`'s own early-stop signal(s); requires `stopIf`. */
  readonly onStop?: Parameters<SequenceFunction["loop"]>[1]["onStop"];
};

export type GuardedProductionHandle = SequenceHandle & {
  /** The first (or only) review seat's verdict slot — `verdicts[0]`. */
  readonly verdict: SlotWithFields<ReviewerVerdictValue>;
  /** Every review seat's verdict slot, in the order `review` declared them. */
  readonly verdicts: readonly SlotWithFields<ReviewerVerdictValue>[];
};

/**
 * The produce-first refinement composite: a bounded loop whose body is
 * `produce → [caller evidence] → review`, exiting once the reviewer's
 * verdict is `approved` (and, when given, `alsoRequire`). Round 1 the
 * verdict slot does not exist yet and is hidden from the producer
 * (`input.optional`, P447); rounds 2+ it is attached, carrying the prior
 * round's blockers back in. Invents exactly one slot, `<id>-verdict` — the
 * artifact slot is caller-declared and passed in as `produces`.
 *
 * Approval is evaluated after the round, so the loop exits before produce
 * could run again: there is no bypass guard here, and none is needed — the
 * touched-after-certification bug class simply has no place to exist in
 * this shape.
 *
 * `produce.text` must not itself interpolate the verdict: the factory
 * already carries it as an optional input, and a caller interpolation of
 * the same ref triggers a `cdk-duplicate-input` diagnostic and drops the
 * round-1-hidden optional semantics.
 * @example
 * ```ts
 * const plan = slot.text("plan");
 * const planning = guardedProduction({
 *   id: "planning",
 *   produces: plan,
 *   produce: { agent: worker, text: prompt.text`Draft a plan.` },
 *   review: { agent: reviewer, text: prompt.text`Review ${plan}.` },
 *   rounds: 3,
 * });
 * ```
 */
export function guardedProduction<Produces>(options: GuardedProductionOptions<Produces>): GuardedProductionHandle {
  const {
    id,
    produces,
    produce,
    review,
    evidence,
    afterReview,
    alsoRequire,
    carry,
    minRounds,
    rounds,
    onExhausted,
    stopIf,
    onStop,
  } = options;
  if (minRounds !== undefined && carry === undefined) {
    throw new Error("guardedProduction: \"minRounds\" requires \"carry\"");
  }
  if (minRounds !== undefined && minRounds <= 1) {
    throw new Error("guardedProduction: \"minRounds\" must be greater than 1");
  }
  if (minRounds !== undefined && minRounds >= rounds) {
    throw new Error("guardedProduction: \"minRounds\" must be less than \"rounds\"");
  }
  const seats = Array.isArray(review) ? review : [review];
  const suffix = (index: number) => (seats.length > 1 ? `-${index + 1}` : "");
  const verdicts = seats.map((seat, index) =>
    seat.verdictSlot
      ?? slot.of({
        id: `${id}-verdict${suffix(index)}`,
        schema: seat.verdictSchema ?? reviewerVerdict,
        description: `Reviewer verdict for the "${id}" produce-review round.`,
      })
  );
  const produceStep = sequence.prompt(`${id}-produce`, {
    agent: produce.agent,
    text: produce.text,
    output: produces,
    input: [...verdicts, ...(produce.optionalInputs ?? [])].map((slotToHide) => input.optional(slotToHide)),
  });
  const reviewSteps = seats.map((seat, index) =>
    sequence.prompt(`${id}-review${suffix(index)}`, {
      agent: seat.agent,
      text: seat.text,
      output: seat.extraOutputs ? [verdicts[index], ...seat.extraOutputs] : verdicts[index],
      ...(seat.extraInputs === undefined ? {} : { input: [...seat.extraInputs] }),
    })
  );
  const sealed = carry === undefined ? undefined : slot.boolean(`${id}-sealed`);
  const body = sequence.linear(
    `${id}-body`,
    carry === undefined
      ? [produceStep, ...(evidence ?? []), ...reviewSteps, ...(afterReview ?? [])]
      : [
        sequence.project(`${id}-unseal`, {
          projections: [{ source: operation.literal(false), destination: sealed! }],
        }),
        produceStep,
        ...(evidence ?? []),
        ...reviewSteps,
        ...(afterReview ?? []),
        sequence.project(`${id}-seal`, {
          projections: [{ source: operation.literal(true), destination: sealed! }],
        }),
      ],
  );
  // A bare `condition.equals` when there is exactly one seat (matches the
  // pre-extension shape byte-for-byte); `condition.all` only wraps when
  // there is a second seat to conjoin — `condition.all([x])` would
  // otherwise emit a `{all: [...]}` guard group where a single-seat caller
  // used to get a bare `equals` predicate.
  const perSeatApproved = verdicts.map((verdict) => condition.equals(verdict.status, "approved"));
  const approved = perSeatApproved.length === 1 ? perSeatApproved[0]! : condition.all(perSeatApproved);
  const until = carry === undefined
    ? alsoRequire === undefined ? approved : condition.all([approved, alsoRequire])
    : condition.all([
      approved,
      ...(alsoRequire === undefined ? [] : [alsoRequire]),
      condition.equals(sealed!, true),
      condition.count(carry).where("needs", []).equals(0),
      ...(minRounds === undefined
        ? []
        : [condition.any([condition.count(carry).equals(0), condition.iterationAtLeast(minRounds - 1)])]),
    ]);
  const loopHandle = sequence.loop(id, {
    sequence: body,
    until,
    iterations: rounds,
    ...(onExhausted === undefined ? {} : { onExhausted }),
    ...(stopIf === undefined ? {} : { stopIf }),
    ...(onStop === undefined ? {} : { onStop }),
  });
  // Not the plain `Object.assign(loopHandle, { verdict })` idiom from
  // `port.ts`/`agent.ts`: those merge onto a plain lookup object, but
  // `loopHandle` IS the canonical sequence-item JSON `sequenceOf` (P485)
  // builds via `withMeta` (never `withDeclaration`, so there is no separate
  // `meta.declaration` for `normalizeValue` to prefer) — an enumerable
  // `verdict`/`verdicts` property would serialize straight into the built
  // canonical as an unknown field. `withHiddenField` (the shared CDK
  // non-enumerable-augmentation helper `sequence.check`'s `pass` also
  // routes through) attaches them for callers without polluting the loop
  // step's own JSON shape.
  const withVerdict = withHiddenField(loopHandle, "verdict", verdicts[0]);
  return withHiddenField(withVerdict, "verdicts", verdicts) as GuardedProductionHandle;
}

export type CommitTailOptions = {
  readonly id: string;
  /** Writes the commit message; reads the verdict first (honest-exhaustion doctrine) and the caller's work summary. A bare agent gets the kit's own scribe prose (unchanged); `{ agent, text }` lets the caller own the scribe prompt text entirely — the kit synthesizes none of it in that case. */
  readonly scribe: AgentHandle | GuardedProductionRole;
  /** The caller's work summary, forwarded to the scribe alongside the verdict. */
  readonly summary: SlotHandle<string>;
  /** The verdict the scribe reads before writing the message — typically a `guardedProduction` round's own `.verdict`. */
  readonly verdict: SlotWithFields<ReviewerVerdictValue>;
  /** Caller-declared slot the commit step's output lands in. */
  readonly receipt: SlotHandle<string>;
};

/**
 * The clean-tree-guarded git tail: `git status --porcelain`, and when the
 * tree is non-empty, scribe writes the message (reading the verdict first)
 * → stage → commit. A clean tree skips the whole tail (preserves P397
 * behavior) — no scribe frame, no commit attempt.
 *
 * Returns the tail's ordered steps rather than one named `sequence.linear`
 * block: no existing trait uses a named linear as a top-level procedure
 * item, and `ProcedureFields.sequence` does not accept a
 * `SequenceLinearHandle` there — this is the documented fallback (today's
 * `guardedCommitTail` shape), costing the tail's "named block" property and
 * nothing else.
 * @example `sequence: [...planning, ...building, ...commitTail({ id: "shipping", scribe, summary: workSummary, verdict: building.verdict, receipt: commitOutput })]`
 */
export function commitTail(options: CommitTailOptions): readonly SequenceHandle[] {
  const { id, scribe, summary, verdict, receipt } = options;
  const status = slot.text({
    id: `${id}-status`,
    description:
      "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
    hint:
      "Empty exactly when the tree is clean; the commit tail is skipped entirely in that case, so no commit is attempted and the scribe never runs.",
  });
  const message = slot.text({
    id: `${id}-message`,
    description: "The commit message the scribe writes for the completed work.",
  });
  // The runtime requires exactly one output slot on every command-backed
  // sequence item; nothing reads this evidence today, but the slot is
  // mandatory, not a `guardedCommitTail`-style optional extra.
  const stageOutput = slot.text({
    id: `${id}-stage-output`,
    description: "Verbatim output of the git-add staging command.",
  });
  const statusStep = sequence.command({
    id: `${id}-status`,
    title: "Check working tree status",
    input: input.command`git status --porcelain`,
    output: status,
  });
  const ownedScribe = "text" in (scribe as Record<string, unknown>) ? (scribe as GuardedProductionRole) : undefined;
  const scribeAgent: AgentHandle = ownedScribe ? ownedScribe.agent : (scribe as AgentHandle);
  const scribeStep = sequence.prompt(`${id}-message`, {
    title: "Write the commit message (scribe)",
    agent: scribeAgent,
    text: ownedScribe
      ? ownedScribe.text
      : prompt.text`
      Read the reviewer verdict ${verdict} first. If its status is revise, say so plainly and end with an "Unresolved reviewer blockers" section quoting each of its blockers verbatim.
      Write a concise commit message for the completed work based on the work summary ${summary} and the verdict.
      Return exactly that message as your output; the runtime injects it into the git commit step.`,
    output: message,
  });
  const stageStep = sequence.command({
    id: `${id}-stage`,
    title: "Stage all changes except runtime state",
    input: input.command`git add -A -- :!.agents/runs`,
    output: stageOutput,
  });
  const commitStep = sequence.command({
    id: `${id}-commit`,
    title: "Commit the work",
    input: input.command`git commit -m ${message}`,
    output: receipt,
  });
  return [
    statusStep,
    sequence.when(`${id}-maybe-commit`, {
      if: condition.not(condition.equals(status, "")),
      // oxlint-disable-next-line unicorn/no-thenable -- `BranchSequenceFields.then` is the CDK's own branch-arm field, not a promise.
      then: [scribeStep, stageStep, commitStep],
    }),
  ];
}
