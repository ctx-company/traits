import type {
  AgentHandle,
  GuardValue,
  OptionalSlotRead,
  PromptTemplate,
  SchemaHandle,
  SequenceFunction,
  SequenceHandle,
  SignalHandle,
  SlotHandle,
  SlotWithFields,
} from "@ctx-traits/cdk";
import { condition, input, operation, sequence, slot, withHiddenField } from "@ctx-traits/cdk";
import type { ReviewerVerdictValue } from "../schema.ts";
import { reviewerVerdict } from "../schema.ts";

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
  /** Optional conditional signal emitted with this review's accepted output. */
  readonly onComplete?: SignalHandle | { readonly signal: SignalHandle; readonly when: GuardValue };
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
  readonly abortIf?: GuardValue;
  /** Forwarded verbatim to `sequence.loop`'s own early-stop signal(s); requires `abortIf`. */
  readonly onAbort?: Parameters<SequenceFunction["loop"]>[1]["onAbort"];
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
 *   produce: { agent: worker, text: input.prompt`Draft a plan.` },
 *   review: { agent: reviewer, text: input.prompt`Review ${plan}.` },
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
    abortIf,
    onAbort,
  } = options;
  if (minRounds !== undefined && carry === undefined) {
    throw new Error('guardedProduction: "minRounds" requires "carry"');
  }
  if (minRounds !== undefined && minRounds <= 1) {
    throw new Error('guardedProduction: "minRounds" must be greater than 1');
  }
  if (minRounds !== undefined && minRounds >= rounds) {
    throw new Error('guardedProduction: "minRounds" must be less than "rounds"');
  }
  const seats = Array.isArray(review) ? review : [review];
  const suffix = (index: number) => (seats.length > 1 ? `-${index + 1}` : "");
  const verdicts = seats.map(
    (seat, index) =>
      seat.verdictSlot ??
      slot({
        id: `${id}-verdict${suffix(index)}`,
        schema: seat.verdictSchema ?? reviewerVerdict,
        description: `Reviewer verdict for the "${id}" produce-review round.`,
      }),
  );
  // The produce step reads its own output slot from the previous round
  // (`input.optional` — absent in round 1, exactly like the verdicts). This
  // is the worker's cross-round memory: without it, every round's worker
  // reconstructed "what has been done so far" by re-auditing the working
  // tree against the draft, which consumed the frame and produced the
  // observed one-small-step-per-round throughput. The reviewer's verdict
  // gets the same self-read below for the same reason: a reviewer that
  // never sees its own prior findings re-derives them from the tree each
  // round, mutating step lists and silently dropping completed work.
  const produceStep = sequence.prompt(`${id}-produce`, {
    agent: produce.agent,
    text: produce.text,
    output: produces,
    input: [
      ...verdicts,
      // `produces` may be one slot or a list; every one is a self-read.
      ...((Array.isArray(produces) ? produces : [produces]) as readonly SlotHandle[]),
      ...(produce.optionalInputs ?? []),
    ].map((slotToHide) => input.optional(slotToHide)),
  });
  const reviewSteps = seats.map((seat, index) =>
    sequence.prompt(`${id}-review${suffix(index)}`, {
      agent: seat.agent,
      text: seat.text,
      output: seat.extraOutputs ? [verdicts[index], ...seat.extraOutputs] : verdicts[index],
      input: [input.optional(verdicts[index]), ...(seat.extraInputs ?? [])],
      ...(seat.onComplete === undefined ? {} : { onComplete: seat.onComplete }),
    }),
  );
  const sealed = carry === undefined ? undefined : slot.boolean(`${id}-sealed`);
  const body = sequence.linear(
    `${id}-body`,
    carry === undefined
      ? [produceStep, ...(evidence ?? []), ...reviewSteps, ...(afterReview ?? [])]
      : [
          sequence.project(`${id}-unseal`, {
            // oxlint-disable-next-line typescript/no-non-null-assertion -- this branch only runs when carry !== undefined, which is exactly when sealed was assigned above
            projections: [{ source: operation.literal(false), destination: sealed! }],
          }),
          produceStep,
          ...(evidence ?? []),
          ...reviewSteps,
          ...(afterReview ?? []),
          sequence.project(`${id}-seal`, {
            // oxlint-disable-next-line typescript/no-non-null-assertion -- see above
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
  // oxlint-disable-next-line typescript/no-non-null-assertion -- length === 1 checked above
  const approved = perSeatApproved.length === 1 ? perSeatApproved[0]! : condition.all(perSeatApproved);
  const until =
    carry === undefined
      ? alsoRequire === undefined
        ? approved
        : condition.all([approved, alsoRequire])
      : condition.all([
          approved,
          ...(alsoRequire === undefined ? [] : [alsoRequire]),
          // oxlint-disable-next-line typescript/no-non-null-assertion -- this branch only runs when carry !== undefined, which is exactly when sealed was assigned above
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
    ...(abortIf === undefined ? {} : { abortIf }),
    ...(onAbort === undefined ? {} : { onAbort }),
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
