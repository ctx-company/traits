import type { CanonicalGuardExpr, CanonicalGuardPredicate, JsonObject, JsonValue, RefKind } from "./generated.js";
import type {
  ConditionHandle,
  FieldRef,
  CheckResultValue,
  GuardHandle,
  PortHandle,
  RefHandle,
  SequenceHandle,
  SettingHandle,
  SlotHandle,
} from "./handles.js";
import type { Meta } from "./meta.js";
import { metaOf, outputSinkDeclaration, withDeclaration, withMeta } from "./meta.js";
import { collectMany, compact, compactAs, normalizeValue, validateSlug } from "./normalize.js";
import type { Mutable } from "./normalize.js";
import { refText } from "./ref.js";
import type { SequenceOutputValue } from "./sequence.js";
import { signalVerbLabel, signalVerbOf } from "./signal-verb.js";

/**
 * The value set a guard position accepts: a bare ref string, a typed
 * `signal:*`/`condition:*` reference (`RefHandle<"condition">` already covers
 * the `condition:*` handle a named `condition.all`/`condition.any`
 * declaration returns — no separate `ConditionHandle` arm is needed), a
 * `condition.*`-built predicate, or a nested array of the same.
 */
export type GuardValue = string | RefHandle<"signal" | "condition"> | GuardHandle | readonly GuardValue[];

/**
 * The unique symbols backing `condition.True`/`condition.False`/
 * `condition.Otherwise`, the `flow.match` arm keys. Minted exactly once,
 * here, and never re-exported — `functional/registrars.ts` imports these
 * consts directly so the symbol identity can't fork.
 */
export const CONDITION_TRUE: unique symbol = Symbol("condition.True");
export const CONDITION_FALSE: unique symbol = Symbol("condition.False");
export const CONDITION_OTHERWISE: unique symbol = Symbol("condition.Otherwise");
/**
 * The check-position value `sequence.branch`'s `check` field accepts: an
 * ordinary guard, or an as-yet-unplaced `sequence.check(...)` step handle
 * used as a one-shot gate. In the latter case the `ok` field of the step's
 * declared verdict-record `pass` output becomes the guard, and the step
 * itself must run immediately before the branch evaluates it (see
 * {@link lowerCheckGuard}).
 */
export type BranchCheckValue = GuardValue | (SequenceHandle & { readonly pass: SlotHandle<CheckResultValue> });
/** Literal or accepted local numeric value used as an ordered-comparison RHS. */
export type NumericComparisonValue = number | SlotHandle<number> | PortHandle<number> | SettingHandle<number>;

/**
 * The chained continuation of `condition.count(slot)`: narrows which
 * elements are counted (`where`) and states the comparison the count must
 * satisfy (`equals`/`atLeast`). Hand-writing this in JSON means composing a
 * `{ count, field, field-equals, equals }` object by hand with no check that
 * the field names line up with the guard grammar the runtime expects.
 * @example
 * ```ts
 * const finding = schema.object("finding", {
 *   severity: schema.field(schema.oneOf("severity", ["blocker", "advisory"])),
 * });
 * const findings = slot.array(finding, "findings");
 * condition.count(findings).where("severity", "blocker").equals(0);
 * ```
 */
export interface ConditionCountFunction {
  /** Terminates the count guard: true when the (optionally filtered) count equals `value`. @example `condition.count(findings).equals(0)` */
  equals(value: CountComparisonValue): GuardHandle;
  /** Terminates the count guard: true when the (optionally filtered) count is at least `value`. @example `condition.count(findings).atLeast(1)` */
  atLeast(value: CountComparisonValue): GuardHandle;
  /**
   * Counts only elements whose `field` equals `value`.
   * @example `condition.count(findings).where("severity", "blocker").equals(0)`
   */
  where(field: string, value: JsonValue): ConditionCountFunction;
}
/** A literal threshold or another closed count operand. */
export type CountComparisonValue = number | ConditionCountFunction;

/**
 * Guard builder methods reachable as `condition.*`, each returning the JSON
 * guard shape a `sequence.when`/`sequence.loop` accepts for `if`/`until`/
 * `abortIf`. The single-line `@example`s on the individual methods below all
 * reuse this declared context:
 * @example
 * ```ts
 * const verdict = slot.text("verdict");
 * const finding = schema.object("finding", {
 *   severity: schema.field(schema.oneOf("severity", ["blocker", "advisory"])),
 * });
 * const findings = slot.array(finding, "findings");
 * const retryCount = slot.number("retry-count");
 * ```
 */
export interface ConditionFunction {
  /**
   * Guards on whether a named signal has fired — the fan-in/fan-out
   * coordination primitive, distinct from a slot value comparison. `signalRef`
   * must resolve to a typed `signal:*` ref (a declared `ref.signal(...)`
   * handle, or an equivalent bare `"signal:<id>"` string); an undeclared
   * bare id fails guard validation at synth time.
   * @example `condition.signal(ref.signal("review-complete"))`
   */
  signal(signalRef: string | RefHandle): GuardHandle;
  /**
   * Guards that a slot's value equals `value` exactly. The everyday
   * `{ slot, equals }` guard: hand-written it's easy to get the key name
   * wrong (`"eq"`, `"value"`); `condition.equals` always emits `equals`.
   * Accepts a typed slot/field ref (constraining `value` to that slot or
   * field's declared value type) or a bare ref string for untyped authoring.
   * Passing a `FieldRef` from `slot.foo` lowers to exactly the same
   * `{ slot, field, equals }` guard `condition.fieldEquals(slot, "foo",
   * value)` emits by hand.
   * @example `condition.equals(verdict, "approved")`
   */
  equals<Value extends JsonValue>(
    target: string | RefHandle<RefKind, Value> | FieldRef<Value>,
    value: NoInfer<Value>,
  ): GuardHandle;
  /**
   * Guards that a boolean value is true — sugar for
   * `condition.equals(target, true)`, the everyday read of a check verdict's
   * `ok` field. Lowers to exactly the same canonical guard, never a separate
   * form.
   * @example `condition.isTrue(gatePassed.ok)`
   */
  isTrue(target: string | RefHandle<RefKind, boolean> | FieldRef<boolean>): GuardHandle;
  /**
   * Negated {@link ConditionFunction.isTrue} — sugar for
   * `condition.equals(target, false)`.
   * @example `condition.isFalse(gatePassed.ok)`
   */
  isFalse(target: string | RefHandle<RefKind, boolean> | FieldRef<boolean>): GuardHandle;
  /**
   * Guards that one *field* of a slot's object value equals `value`, without
   * reading the whole object out first. `field` is a plain string with no
   * static check against the slot's declared schema.
   * @example `condition.fieldEquals(verdict, "status", "approved")`
   */
  fieldEquals(slotRef: string | SlotHandle | SettingHandle | RefHandle, field: string, value: JsonValue): GuardHandle;
  /**
   * Negates a guard — wraps any `condition.*` result (or a nested
   * `all`/`any`) so the branch or loop reads as "until NOT this".
   * @example `condition.not(condition.equals(verdict, "approved"))`
   */
  not(guard: GuardValue): GuardHandle;
  /**
   * Guards that a slot holding a list or string is empty, without a
   * separate `condition.count(...).equals(0)` for the common zero-length
   * case.
   * @example `condition.empty(findings)`
   */
  empty(slotRef: string | SlotHandle | SettingHandle | RefHandle): GuardHandle;
  /**
   * Negated {@link ConditionFunction.empty} — lowers to
   * `condition.not(condition.empty(...))`, never a separate canonical form.
   * The everyday maybe-commit guard.
   * @example `condition.notEmpty(gitStatus)`
   */
  notEmpty(slotRef: string | SlotHandle | SettingHandle | RefHandle): GuardHandle;
  /**
   * Starts a count guard over a list-valued slot: chain `.where(...)` to
   * filter which elements count, then `.equals`/`.atLeast` to state the
   * threshold. See {@link ConditionCountFunction}.
   * @example `condition.count(findings).equals(0)`
   */
  count(slotRef: string | SlotHandle | SettingHandle | RefHandle): ConditionCountFunction;
  /**
   * Guards on whether an optional local input `port:*` was supplied — or,
   * with `field`, whether a declared optional field of a `port:*`/`slot:*`
   * object container is present on it. Tri-state under the hood
   * (`present`/`absent`/unmeasurable) but the routed guard is always a plain
   * boolean; requires the trait to declare `schema-version = "0.3"` or newer.
   * @example `condition.present(costCap)`
   * @example `condition.present(capReport, { field: "cost-report" })`
   */
  present(
    subjectRef: string | PortHandle | SlotHandle | SettingHandle | RefHandle,
    options?: { readonly field?: string },
  ): GuardHandle;
  /**
   * Negated {@link ConditionFunction.present} — lowers to
   * `condition.not(condition.present(...))`, never a separate canonical
   * form.
   * @example `condition.absent(costCap)`
   */
  absent(
    subjectRef: string | PortHandle | SlotHandle | SettingHandle | RefHandle,
    options?: { readonly field?: string },
  ): GuardHandle;
  /** Guards that a slot's numeric value is less than a literal or accepted numeric slot/input-port value. @example `condition.lt(retryCount, 3)` */
  lt(slotRef: string | SlotHandle | SettingHandle | RefHandle, value: NumericComparisonValue): GuardHandle;
  /** Guards that a slot's numeric value is at most `value`. @example `condition.lte(retryCount, 3)` */
  lte(slotRef: string | SlotHandle | SettingHandle | RefHandle, value: NumericComparisonValue): GuardHandle;
  /** Guards that a slot's numeric value is greater than `value`. @example `condition.gt(retryCount, 7)` */
  gt(slotRef: string | SlotHandle | SettingHandle | RefHandle, value: NumericComparisonValue): GuardHandle;
  /** Guards that a slot's numeric value is at least `value`. @example `condition.gte(retryCount, 7)` */
  gte(slotRef: string | SlotHandle | SettingHandle | RefHandle, value: NumericComparisonValue): GuardHandle;
  /**
   * Guards that cumulative active-drive elapsed seconds — runtime-supplied
   * evidence measured by the CLI drive across resumes, never a value any
   * step writes — is at least `value`. Closed guard form: there is no
   * matching `slot`/`output` to read the elapsed value from directly, only
   * this comparison. Typically paired with a target-reached guard in a
   * loop's `until` to express "stop when the goal is met OR time is up".
   * @example `condition.elapsedAtLeast(timeLimitSeconds)`
   */
  elapsedAtLeast(value: NumericComparisonValue): GuardHandle;
  /** Ordered comparison over one top-level numeric field of an inline object-schema slot. The Rust validator checks the named field and RHS ref schemas. */
  fieldLt(
    slotRef: string | SlotHandle | SettingHandle | RefHandle,
    field: string,
    value: NumericComparisonValue,
  ): GuardHandle;
  /** Ordered `<=` comparison over one top-level numeric object field. */
  fieldLte(
    slotRef: string | SlotHandle | SettingHandle | RefHandle,
    field: string,
    value: NumericComparisonValue,
  ): GuardHandle;
  /** Ordered `>` comparison over one top-level numeric object field. */
  fieldGt(
    slotRef: string | SlotHandle | SettingHandle | RefHandle,
    field: string,
    value: NumericComparisonValue,
  ): GuardHandle;
  /** Ordered `>=` comparison over one top-level numeric object field. */
  fieldGte(
    slotRef: string | SlotHandle | SettingHandle | RefHandle,
    field: string,
    value: NumericComparisonValue,
  ): GuardHandle;
  /**
   * Guards that the current loop iteration equals `n` — the closed count of
   * completed iterations, first-class in the canonical guard grammar
   * (`iteration`) but with no bare-ref/string authoring form.
   * @example `condition.iteration(3)`
   */
  iteration(n: number): GuardHandle;
  /**
   * Guards that the current loop iteration is at least `n`. Canonical
   * `iteration-at-least` counterpart to {@link ConditionFunction.iteration}.
   * @example `condition.iterationAtLeast(3)`
   */
  iterationAtLeast(n: number): GuardHandle;
  output: ConditionOutputFunction;
  /**
   * Combines several guards with AND. Called with only an items array it's
   * an inline guard; called with a leading `id` it becomes a named
   * `[condition.*]` declaration other guards can reference via
   * `ref.condition(id)`.
   * @example `condition.all([condition.equals(verdict, "approved"), condition.empty(findings)])`
   */
  all(items: readonly GuardValue[]): GuardHandle;
  all(id: string, items: readonly GuardValue[], fields?: { readonly description?: string }): ConditionHandle;
  /**
   * Combines several guards with OR. Same inline-vs-declared shape as
   * `condition.all`.
   * @example `condition.any([condition.equals(verdict, "fail"), condition.empty(findings)])`
   */
  any(items: readonly GuardValue[]): GuardHandle;
  any(id: string, items: readonly GuardValue[], fields?: { readonly description?: string }): ConditionHandle;
  /**
   * The one sanctioned raw-JSON guard escape: spread `json` directly into the
   * emitted guard predicate for canonical guard fields no typed builder above
   * covers (yet). Prefer a typed `condition.*` builder whenever one exists —
   * the guard-level counterpart to `trait(...)`'s `unsafeJson`.
   * @example `condition.unsafe({ condition: "condition:other" })`
   */
  unsafe(json: JsonObject): GuardHandle;
  /**
   * The `flow.match` arm key for a guard subject's true branch.
   * @example `flow.match("route", check, { [condition.True]: () => {...} })`
   */
  readonly True: typeof CONDITION_TRUE;
  /**
   * The `flow.match` arm key for a guard subject's false branch.
   * @example `flow.match("route", check, { [condition.False]: () => {...} })`
   */
  readonly False: typeof CONDITION_FALSE;
  /**
   * The `flow.match` arm key for a field subject's fallback branch, run when
   * no value arm matched.
   * @example `flow.match("route", status, { approved: () => {...}, [condition.Otherwise]: () => {...} })`
   */
  readonly Otherwise: typeof CONDITION_OTHERWISE;
}

/**
 * Guards compared against a sequence step's own output rather than against a
 * named slot — the shorthand `condition.output.is(value)` reads as "this
 * step's output equals `value`" without declaring which output. `tsc`
 * accepts the one-argument form regardless of how many outputs the step
 * declares; whether that implicit reference is actually unambiguous is
 * checked while lowering the attached guard (Rust synth), not by `tsc`. Pass
 * the output handle explicitly (`condition.output.is(out, value)`) when a
 * step has more than one output, so the reference stays unambiguous.
 * @example `condition.output.is("approved")`
 */
export interface ConditionOutputFunction {
  /**
   * Guards that the (single, implicit) step output equals `value`, or that a
   * named output equals it when the step has more than one — see the
   * one-output-vs-many caveat on {@link ConditionOutputFunction} itself.
   * @example `condition.output.is("approved")`
   */
  is(value: JsonValue): GuardHandle;
  is(outputRef: SequenceOutputValue, value: JsonValue): GuardHandle;
  /**
   * Guards that one field of a named step output equals `value` — the
   * `condition.output` analogue of `condition.fieldEquals`.
   * @example
   * ```ts
   * const review = slot.any("review");
   * sequence.prompt("review", { text: input.prompt`Review the diff.`, output: review });
   * condition.output.fieldIs(review, "status", "approved");
   * ```
   */
  fieldIs(outputRef: SequenceOutputValue, field: string, value: JsonValue): GuardHandle;
  /** Guards that the step's numeric output is less than `value`. @example `condition.output.lt(3)` */
  lt(value: NumericComparisonValue): GuardHandle;
  lt(outputRef: SequenceOutputValue, value: NumericComparisonValue): GuardHandle;
  /** Guards that the step's numeric output is at most `value`. @example `condition.output.lte(3)` */
  lte(value: NumericComparisonValue): GuardHandle;
  lte(outputRef: SequenceOutputValue, value: NumericComparisonValue): GuardHandle;
  /** Guards that the step's numeric output is greater than `value`. @example `condition.output.gt(7)` */
  gt(value: NumericComparisonValue): GuardHandle;
  gt(outputRef: SequenceOutputValue, value: NumericComparisonValue): GuardHandle;
  /** Guards that the step's numeric output is at least `value`. @example `condition.output.gte(7)` */
  gte(value: NumericComparisonValue): GuardHandle;
  gte(outputRef: SequenceOutputValue, value: NumericComparisonValue): GuardHandle;
}

/**
 * Creates inline guards and named condition declarations: the boolean
 * expressions that gate a `sequence.when` branch or a `sequence.loop`'s
 * `until`/`abortIf`.
 *
 * A guard written as raw JSON is a `{ slot, equals }`-shaped object with no
 * static check that the slot exists, that its schema even has the field
 * you're comparing, or that you spelled the comparison operator the
 * validator expects (`"at-least"` vs `"atLeast"` vs `"gte"`) — errors that
 * only surface at synth or run time. `condition.*` spells the comparison
 * operator for you (`condition.gte` always emits `"at-least"`) and, when you
 * pass an actual `slot(...)`/`output(...)` handle instead of a bare ref
 * string, a rename of that declaration is a `tsc` error at the call site —
 * but the *field* name in `fieldEquals`/`count.where` is still a plain
 * string with no static check against the slot's schema, and `slotRef`
 * itself still accepts a bare ref string that bypasses handle checking
 * entirely. Whether the slot/field genuinely exists and the value's type
 * matches is validated by the Rust synth/runtime, not `tsc`.
 *
 * `condition.equals`/`fieldEquals`/`lt`/`gt`/... compare against a
 * procedure-local *slot* (see `slot.ts`). `condition.output.*` compares
 * against the *output* of the sequence step the guard is attached to
 * (`sequence.loop`'s own step output, for instance) without naming a slot at
 * all. `condition.all`/`condition.any` combine several guards, and can
 * either be inlined at the point of use or declared once with an id and
 * referenced elsewhere via `ref.condition`.
 *
 * Every builder below returns a `GuardHandle`: a branded value backed by the
 * closed canonical guard-predicate shape. A guard position (`if`, `when`,
 * `until`, `abort-if`, quorum `guard`) never accepts a raw object literal —
 * a typo'd predicate key (`eqals`, `atLeast` vs `at-least`) is a `tsc` error
 * at the call site instead of a silently-inert or synth-time failure.
 * `condition.unsafe(json)` is the one sanctioned raw-JSON escape, for
 * canonical guard fields no typed builder above covers.
 *
 * @param signalRef A typed signal reference for `signal`.
 * @example
 * ```ts
 * until: condition.all([
 *   condition.fieldEquals(verdict1, "status", "approved"),
 *   condition.fieldEquals(verdict2, "status", "approved"),
 * ])
 * ```
 * @see {@link sequence}
 */
export const condition: ConditionFunction = {
  signal: (signalRef: string | RefHandle): GuardHandle => {
    const verbName = signalVerbOf(signalRef);
    if (verbName !== undefined) {
      throw new Error(
        `condition.signal(${signalVerbLabel(verbName)}) — signal verbs are raise-only; observation is reserved, ` +
          "not undefined. Guard on a declared signal(...) handle instead.",
      );
    }
    return guardHandle(
      { signal: refText(signalRef, "condition.signal") },
      {
        declarations: collectMany([signalRef]),
      },
    );
  },
  equals: function equals<Value extends JsonValue>(
    target: string | RefHandle<RefKind, Value> | FieldRef<Value>,
    value: Value,
  ): GuardHandle {
    const fieldRef = metaOf(target)?.fieldRef;
    if (fieldRef !== undefined) {
      return guardHandle(
        { slot: fieldRef.slotRef, field: fieldRef.field, equals: value },
        {
          declarations: collectMany([target]),
        },
      );
    }
    return guardHandle(
      { slot: refText(target, "condition.slot"), equals: value },
      {
        declarations: collectMany([target]),
      },
    );
  },
  isTrue: (target: string | RefHandle<RefKind, boolean> | FieldRef<boolean>): GuardHandle =>
    condition.equals(target, true),
  isFalse: (target: string | RefHandle<RefKind, boolean> | FieldRef<boolean>): GuardHandle =>
    condition.equals(target, false),
  fieldEquals: (
    slotRef: string | SlotHandle | SettingHandle | RefHandle,
    field: string,
    value: JsonValue,
  ): GuardHandle =>
    guardHandle(
      { slot: refText(slotRef, "condition.slot"), field, equals: value },
      {
        declarations: collectMany([slotRef]),
      },
    ),
  not: (guard: GuardValue): GuardHandle =>
    guardHandle(
      { not: normalizeGuard(guard) },
      {
        declarations: collectMany([guard]),
        defaultOutputPaths: defaultOutputPaths(guard).map((path) => ["not", ...path]),
      },
    ),
  empty: (slotRef: string | SlotHandle | SettingHandle | RefHandle): GuardHandle =>
    guardHandle(
      { empty: refText(slotRef, "condition.empty") },
      {
        declarations: collectMany([slotRef]),
      },
    ),
  count: (slotRef: string | SlotHandle | SettingHandle | RefHandle): ConditionCountFunction => countFunction(slotRef),
  notEmpty: (slotRef: string | SlotHandle | SettingHandle | RefHandle): GuardHandle =>
    condition.not(condition.empty(slotRef)),
  present: (
    subjectRef: string | PortHandle | SlotHandle | SettingHandle | RefHandle,
    options?: { readonly field?: string },
  ): GuardHandle =>
    guardHandle(
      { present: refText(subjectRef, "condition.present"), field: options?.field },
      {
        declarations: collectMany([subjectRef]),
      },
    ),
  absent: (
    subjectRef: string | PortHandle | SlotHandle | SettingHandle | RefHandle,
    options?: { readonly field?: string },
  ): GuardHandle => condition.not(condition.present(subjectRef, options)),
  lt: (slotRef: string | SlotHandle | SettingHandle | RefHandle, value: NumericComparisonValue): GuardHandle =>
    slotComparison(slotRef, "less-than", value),
  lte: (slotRef: string | SlotHandle | SettingHandle | RefHandle, value: NumericComparisonValue): GuardHandle =>
    slotComparison(slotRef, "at-most", value),
  gt: (slotRef: string | SlotHandle | SettingHandle | RefHandle, value: NumericComparisonValue): GuardHandle =>
    slotComparison(slotRef, "greater-than", value),
  gte: (slotRef: string | SlotHandle | SettingHandle | RefHandle, value: NumericComparisonValue): GuardHandle =>
    slotComparison(slotRef, "at-least", value),
  elapsedAtLeast: (value: NumericComparisonValue): GuardHandle =>
    guardHandle(
      { "elapsed-seconds-at-least": comparisonValue(value) },
      {
        declarations: collectMany([value]),
      },
    ),
  fieldLt: (slotRef, field, value): GuardHandle => slotComparison(slotRef, "less-than", value, field),
  fieldLte: (slotRef, field, value): GuardHandle => slotComparison(slotRef, "at-most", value, field),
  fieldGt: (slotRef, field, value): GuardHandle => slotComparison(slotRef, "greater-than", value, field),
  fieldGte: (slotRef, field, value): GuardHandle => slotComparison(slotRef, "at-least", value, field),
  iteration: (n: number): GuardHandle => guardHandle({ iteration: n }),
  iterationAtLeast: (n: number): GuardHandle => guardHandle({ "iteration-at-least": n }),
  output: {
    is: (...args: readonly [JsonValue] | readonly [SequenceOutputValue, JsonValue]): GuardHandle =>
      args.length === 1
        ? guardHandle({ equals: args[0] }, { defaultOutput: true })
        : guardHandle(
            { output: outputRefText(args[0], "condition.output"), equals: args[1] },
            {
              declarations: collectMany([args[0]]),
            },
          ),
    fieldIs: (outputRef: SequenceOutputValue, field: string, value: JsonValue): GuardHandle =>
      guardHandle(
        { output: outputRefText(outputRef, "condition.output"), field, equals: value },
        {
          declarations: collectMany([outputRef]),
        },
      ),
    lt: (
      ...args: readonly [NumericComparisonValue] | readonly [SequenceOutputValue, NumericComparisonValue]
    ): GuardHandle => outputComparison("less-than", args),
    lte: (
      ...args: readonly [NumericComparisonValue] | readonly [SequenceOutputValue, NumericComparisonValue]
    ): GuardHandle => outputComparison("at-most", args),
    gt: (
      ...args: readonly [NumericComparisonValue] | readonly [SequenceOutputValue, NumericComparisonValue]
    ): GuardHandle => outputComparison("greater-than", args),
    gte: (
      ...args: readonly [NumericComparisonValue] | readonly [SequenceOutputValue, NumericComparisonValue]
    ): GuardHandle => outputComparison("at-least", args),
  },
  all: conditionAll,
  any: conditionAny,
  unsafe: (json: JsonObject): GuardHandle => guardHandle(json as Mutable<CanonicalGuardPredicate>),
  True: CONDITION_TRUE,
  False: CONDITION_FALSE,
  Otherwise: CONDITION_OTHERWISE,
};

/**
 * Lowers a `BranchCheckValue` to a guard plus, when `value` is an unplaced
 * `sequence.check(...)` step, that step as a gate that must run immediately
 * before the guard is evaluated. The single shared home for check-as-guard
 * polymorphism (CDK grammar v2 rule 6): `sequence.branch`'s `check` field
 * routes through this, and a future loop `until`/`abortIf` rider reuses it
 * rather than re-implementing the discrimination.
 */
export function lowerCheckGuard(
  value: BranchCheckValue,
  path: string,
): { readonly guard: GuardValue; readonly gateStep?: SequenceHandle } {
  if (metaOf(value)?.kind === "sequence-step") {
    const pass = (value as { readonly pass?: SlotHandle<CheckResultValue> }).pass;
    if (pass === undefined) {
      throw new Error(
        `${path}: a sequence step used as a check must declare a verdict-record pass output (see sequence.check)`,
      );
    }
    // P565: the verdict is the `ok` field of the check's record output, not
    // the record itself — `equals(record, true)` can never route true.
    return { guard: condition.fieldEquals(pass, "ok", true), gateStep: value as SequenceHandle };
  }
  return { guard: value as GuardValue };
}

/**
 * Shared producer for every `condition.*` builder: compacts an
 * incrementally-assembled `Mutable<CanonicalGuardPredicate>` body into the
 * closed readonly canonical shape and attaches guard metadata. The one reuse
 * point that keeps `condition.ts` free of `as JsonObject` casts — every
 * builder above routes through it instead of hand-rolling `withMeta`.
 */
function guardHandle(predicate: Mutable<CanonicalGuardPredicate>, meta: Omit<Meta, "kind"> = {}): GuardHandle {
  return withMeta(compactAs<CanonicalGuardPredicate>(predicate), { kind: "guard", ...meta });
}

type ComparisonField = "less-than" | "at-most" | "greater-than" | "at-least";

interface CountFilter {
  readonly field: string;
  readonly "field-equals": JsonValue;
}

interface CountSpec {
  readonly count: string;
  readonly filter?: CountFilter;
  readonly source: string | SlotHandle | SettingHandle | RefHandle;
}

const countSpecs = new WeakMap<object, CountSpec>();

function countFunction(
  slotRef: string | SlotHandle | SettingHandle | RefHandle,
  filter?: CountFilter,
): ConditionCountFunction {
  const result: ConditionCountFunction = {
    equals: (value: CountComparisonValue): GuardHandle => countGuard(slotRef, "equals", value, filter),
    atLeast: (value: CountComparisonValue): GuardHandle => countGuard(slotRef, "at-least", value, filter),
    where: (field: string, value: JsonValue): ConditionCountFunction =>
      countFunction(slotRef, { field, "field-equals": value }),
  };
  countSpecs.set(result, {
    count: refText(slotRef, "condition.count"),
    ...(filter === undefined ? {} : { filter }),
    source: slotRef,
  });
  return result;
}

function countGuard(
  slotRef: string | SlotHandle | SettingHandle | RefHandle,
  modifier: "equals" | "at-least",
  value: CountComparisonValue,
  filter?: CountFilter,
): GuardHandle {
  const predicate: Mutable<CanonicalGuardPredicate> = {
    count: refText(slotRef, "condition.count"),
    field: filter?.field,
    "field-equals": filter?.["field-equals"],
  };
  const rhs = typeof value === "number" ? value : countSpecs.get(value);
  if (rhs === undefined) throw new Error("condition.count: RHS must be another condition.count(...) chain");
  predicate[modifier] =
    typeof rhs === "number"
      ? rhs
      : compact({ count: rhs.count, field: rhs.filter?.field, "field-equals": rhs.filter?.["field-equals"] });
  return guardHandle(predicate, {
    declarations: typeof rhs === "number" ? collectMany([slotRef]) : collectMany([slotRef, rhs.source]),
  });
}

function slotComparison(
  slotRef: string | SlotHandle | SettingHandle | RefHandle,
  modifier: ComparisonField,
  value: NumericComparisonValue,
  field?: string,
): GuardHandle {
  const predicate: Mutable<CanonicalGuardPredicate> = { slot: refText(slotRef, "condition.slot"), field };
  predicate[modifier] = comparisonValue(value);
  return guardHandle(predicate, { declarations: collectMany([slotRef, value]) });
}

function outputComparison(
  modifier: ComparisonField,
  args: readonly [NumericComparisonValue] | readonly [SequenceOutputValue, NumericComparisonValue],
): GuardHandle {
  if (args.length === 1) {
    const predicate: Mutable<CanonicalGuardPredicate> = {};
    predicate[modifier] = comparisonValue(args[0]);
    return guardHandle(predicate, { defaultOutput: true, declarations: collectMany([args[0]]) });
  }
  const predicate: Mutable<CanonicalGuardPredicate> = { output: outputRefText(args[0], "condition.output") };
  predicate[modifier] = comparisonValue(args[1]);
  return guardHandle(predicate, { declarations: collectMany([args[0], args[1]]) });
}

function comparisonValue(value: NumericComparisonValue): JsonValue {
  return typeof value === "number" ? value : { ref: refText(value, "condition.comparison.ref") };
}

function conditionAll(items: readonly GuardValue[]): GuardHandle;
function conditionAll(
  id: string,
  items: readonly GuardValue[],
  fields?: { readonly description?: string },
): ConditionHandle;
function conditionAll(
  idOrItems: string | readonly GuardValue[],
  maybeItems?: readonly GuardValue[],
  fields?: { readonly description?: string },
): GuardHandle | ConditionHandle {
  return conditionGroup("all", idOrItems, maybeItems, fields);
}
function conditionAny(items: readonly GuardValue[]): GuardHandle;
function conditionAny(
  id: string,
  items: readonly GuardValue[],
  fields?: { readonly description?: string },
): ConditionHandle;
function conditionAny(
  idOrItems: string | readonly GuardValue[],
  maybeItems?: readonly GuardValue[],
  fields?: { readonly description?: string },
): GuardHandle | ConditionHandle {
  return conditionGroup("any", idOrItems, maybeItems, fields);
}
function conditionGroup(
  kind: "all" | "any",
  idOrItems: string | readonly GuardValue[],
  maybeItems: readonly GuardValue[] | undefined,
  maybeFields: { readonly description?: string } | undefined,
): GuardHandle | ConditionHandle {
  const items = typeof idOrItems === "string" ? maybeItems : idOrItems;
  const declarations = collectMany(items ?? []);
  const outputPaths = (items ?? []).flatMap((item, index) =>
    defaultOutputPaths(item).map((path) => [kind, index, ...path] as const),
  );
  const normalizedItems = normalizeGuardList(items ?? []);
  if (typeof idOrItems === "string") {
    validateSlug(idOrItems, "condition.id");
    const fields = maybeFields ?? {};
    const declaration = compact({
      id: idOrItems,
      [kind]: normalizedItems,
      description: fields.description,
    });
    return withDeclaration(
      "condition",
      `condition:${idOrItems}`,
      declaration,
      {},
      {
        declarations,
        defaultOutputPaths: outputPaths,
      },
    );
  }
  const predicate: Mutable<CanonicalGuardPredicate> = {};
  predicate[kind] = normalizedItems;
  return guardHandle(predicate, { declarations, defaultOutputPaths: outputPaths });
}

export function defaultOutputPaths(value: GuardValue | undefined): readonly (readonly (string | number)[])[] {
  if (value === undefined) return [];
  if (Array.isArray(value)) {
    return value.flatMap((item, index) => defaultOutputPaths(item).map((path) => [index, ...path]));
  }
  const meta = metaOf(value);
  if (meta?.defaultOutputPaths !== undefined) return meta.defaultOutputPaths;
  return meta?.defaultOutput === true ? [[]] : [];
}

/** Normalizes a guard list, dropping the (unreachable for defined items) `undefined` arm `normalizeGuard`'s signature carries for its own recursive/optional callers. */
function normalizeGuardList(items: readonly GuardValue[]): readonly CanonicalGuardExpr[] {
  return items.map((item) => normalizeGuard(item)).filter((item): item is CanonicalGuardExpr => item !== undefined);
}

export function normalizeGuard(value: GuardValue | undefined): CanonicalGuardExpr | undefined {
  if (value === undefined) return undefined;
  if (Array.isArray(value)) {
    return normalizeGuardList(value);
  }
  const meta = metaOf(value);
  if (meta?.ref !== undefined && (meta.kind === "condition" || meta.kind === "signal" || meta.kind === "ref")) {
    return meta.ref;
  }
  return normalizeValue(value) as CanonicalGuardExpr;
}

function outputRefText(value: SequenceOutputValue, fieldPath: string): string {
  const declaration = outputSinkDeclaration(value);
  return declaration !== undefined ? declaration.slot : refText(value, fieldPath);
}
