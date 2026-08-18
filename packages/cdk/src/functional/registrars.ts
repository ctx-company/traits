/**
 * The functional registration DSL (0106): `step.*`, `flow.*`, `effect.*`,
 * plus the lowering installed into `context.ts` for `agent.prompt`/
 * `slot.forEach`. Every emission here goes through an existing object-layer
 * constructor (`sequence.*`) — this module never builds canonical JSON by
 * hand (0106 Watch: "the functional layer may not reach around the object
 * layer").
 */
import type { BranchCheckValue, GuardValue } from "../condition.js";
import { CONDITION_FALSE, CONDITION_OTHERWISE, CONDITION_TRUE, condition, lowerCheckGuard } from "../condition.js";
import type { JsonValue } from "../generated.js";
import type {
  CheckResultValue,
  FieldRef,
  RefHandle,
  SequenceHandle,
  SequenceLinearHandle,
  SettingHandle,
  SlotHandle,
} from "../handles.js";
import type {
  CheckSequenceFields,
  CommandSequenceFields,
  ExhaustionPolicy,
  ExhaustionSignalValue,
  ForEachSequenceFields,
  LoopSequenceFields,
  ParallelBranchFailurePolicy,
  ParallelOptions,
  ProjectSequenceFields,
  SignalOutputValue,
} from "../sequence.js";
import { metaOf } from "../meta.js";
import type { SchemaValue } from "../schema.js";
import { idFromTitle, sequence, validateNoDuplicateTitles } from "../sequence.js";
import type { TerminalBinding } from "../sequence.js";
import type { SessionTitleSinkInput } from "../sink.js";
import { slot } from "../slot.js";
import { signalVerbLabel, signalVerbOf } from "../signal-verb.js";
import type { SignalVerb, SignalVerbName } from "../signal-verb.js";
import type { AuthorFrame, RegisteredItem, Scope } from "./context.js";
import {
  captureAuthorFrame,
  currentScope,
  declareSessionTitleSink,
  installAgentPromptLowering,
  installSlotForEachLowering,
  nearestScope,
  registerItem,
  runInScope,
} from "./context.js";

function mintId(title: string): string {
  return idFromTitle(title);
}

function buildError(title: string | undefined, rule: string, frame: AuthorFrame | undefined): Error {
  const subject = title === undefined ? "(untitled)" : JSON.stringify(title);
  const prefix = frame === undefined ? "" : `${frame.file}: `;
  return new Error(`${prefix}step titled ${subject} — ${rule}`);
}

/**
 * Site-legality mapping for a `signal.*` verb (0209): resolves `value`
 * through `legal` (verb name -> this site's canonical string). Shared by
 * every registrar that turns a raised verb into a stored policy value, so
 * the brand check (`signalVerbOf`) lives exactly once; each call site still
 * phrases its own error (`buildError` inside a step/container,  plain
 * `Error` for a scope-less registrar like `effect.onComplete`).
 */
function mapLegalVerb<Mapped extends string>(
  value: unknown,
  legal: Readonly<Partial<Record<SignalVerbName, Mapped>>>,
): { readonly verbName: SignalVerbName | undefined; readonly mapped: Mapped | undefined; readonly allowed: string } {
  const verbName = signalVerbOf(value);
  const allowed = (Object.keys(legal) as SignalVerbName[]).map(signalVerbLabel).join(" or ");
  return { verbName, mapped: verbName === undefined ? undefined : legal[verbName], allowed };
}

/** `mapLegalVerb` for a step/container registrar — throws `buildError`, naming the site and title. */
function requireLegalVerb<Mapped extends string>(
  value: unknown,
  legal: Readonly<Partial<Record<SignalVerbName, Mapped>>>,
  siteLabel: string,
  title: string | undefined,
  frame: AuthorFrame | undefined,
): Mapped {
  const { verbName, mapped, allowed } = mapLegalVerb(value, legal);
  if (verbName === undefined) {
    throw buildError(title, `${siteLabel} requires ${allowed} — got ${JSON.stringify(value)}`, frame);
  }
  if (mapped === undefined) {
    throw buildError(
      title,
      `${siteLabel}: ${signalVerbLabel(verbName)} is not legal here — allowed: ${allowed}`,
      frame,
    );
  }
  return mapped;
}

/** Re-runs the 0104 collision rule at scope close, so a build error surfaces at authoring time instead of waiting for `procedure()`'s own re-check. */
function checkDuplicateTitles(items: readonly RegisteredItem[], scopeLabel: string): void {
  validateNoDuplicateTitles(
    items.map((registered) => registered.item as SequenceHandle),
    scopeLabel,
  );
}

function itemsOf(scope: Scope): readonly SequenceHandle[] {
  return scope.items.map((registered) => registered.item as SequenceHandle);
}

/**
 * Positional lowering (0102/0106): anything about to be registered after
 * `flow.until` in its OWN loop scope (not a nested block scope) must carry
 * `not(cond)` — every container kind composes or refuses via this shared
 * guard, so none can silently skip the checkpoint (reviewer verdict on 0106
 * round 1: `flow.loop`/`items.forEach`/`flow.when` leaves were wrapped, but
 * container items registered into the loop scope were not).
 */
function positionalUntilGuard(): GuardValue | undefined {
  const loopScope = nearestScope("loop");
  if (loopScope?.loop?.untilCondition === undefined) return undefined;
  if (currentScope("flow positional lowering") !== loopScope) return undefined;
  return condition.not(lowerCheckGuard(loopScope.loop.untilCondition, "flow.until").guard);
}

/** Composes `positionalUntilGuard()` into `fields.when`, for container kinds whose object-layer constructor accepts `when` directly. */
function withPositionalWhen<F extends { readonly when?: GuardValue }>(fields: F): F {
  const notUntil = positionalUntilGuard();
  if (notUntil === undefined) return fields;
  return { ...fields, when: fields.when === undefined ? notUntil : condition.all([notUntil, fields.when]) };
}

function requireBuild(caller: string): void {
  // `currentScope` throws the exact "called outside procedure.from" message when no build is active.
  currentScope(caller);
}

/**
 * Refuses a container kind whose object-layer constructor cannot carry the
 * positional `not(until)` guard without changing arm routing (`flow.match`,
 * `flow.parallel`) — silence is the one unacceptable outcome (0106 review),
 * so registering one of these directly into a loop scope after `flow.until`
 * is a loud build error instead of a silently unguarded emission.
 */
function forbidPositionalUntil(title: string, containerLabel: string, frame: AuthorFrame | undefined): void {
  if (positionalUntilGuard() === undefined) return;
  throw buildError(
    title,
    `${containerLabel} registered after flow.until in this loop cannot be guarded without changing arm routing — ` +
      "restructure so it is not positioned after flow.until in the enclosing loop",
    frame,
  );
}

/** Common optional override on every `step.*`/`agent.*.prompt` registrar (0109 F2): the emitted canonical id, when it must differ from `idFromTitle(title)`. */
export interface IdOverride {
  readonly id?: string;
}

// ---------------------------------------------------------------------------
// step.*
// ---------------------------------------------------------------------------

export const step = {
  command(
    title: string,
    opts: Omit<CommandSequenceFields, "id" | "kind" | "title"> & IdOverride = {} as never,
  ): SequenceHandle {
    requireBuild(`step.command(${JSON.stringify(title)})`);
    const id = opts.id ?? mintId(title);
    const fields = withPositionalWhen({ ...opts, title });
    const item = sequence.command(id, fields as Omit<CommandSequenceFields, "id" | "kind">);
    registerItem(`step.command(${JSON.stringify(title)})`, item, title);
    return item;
  },
  check(
    title: string,
    opts: Omit<CheckSequenceFields, "id" | "kind" | "title"> & IdOverride,
  ): SequenceHandle & { readonly pass: SlotHandle<CheckResultValue> } {
    requireBuild(`step.check(${JSON.stringify(title)})`);
    const id = opts.id ?? mintId(title);
    const fields = withPositionalWhen({ ...opts, title });
    const item = sequence.check(id, fields as Omit<CheckSequenceFields, "id" | "kind">);
    registerItem(`step.check(${JSON.stringify(title)})`, item, title);
    return item;
  },
  /** `step.project` (0109 F3): the functional home for a deterministic, zero-prompt `project` step — mirrors `sequence.project`'s `projections` field, with the same `id:` override escape as every other `step.*` registrar. */
  project(title: string, opts: Omit<ProjectSequenceFields, "id" | "kind" | "title"> & IdOverride): SequenceHandle {
    requireBuild(`step.project(${JSON.stringify(title)})`);
    const frame = captureAuthorFrame();
    forbidPositionalUntil(title, "step.project", frame);
    const id = opts.id ?? mintId(title);
    const item = sequence.project(id, { ...opts, title } as Omit<ProjectSequenceFields, "id" | "kind">);
    registerItem(`step.project(${JSON.stringify(title)})`, item, title);
    return item;
  },
};

// ---------------------------------------------------------------------------
// agent.prompt / items.forEach lowering (installed into context.ts)
// ---------------------------------------------------------------------------

installAgentPromptLowering((agentHandle, title, promptOpts) => {
  requireBuild(`agent.prompt(${JSON.stringify(title)})`);
  const id = promptOpts.id ?? mintId(title);
  const fields = withPositionalWhen({ ...promptOpts, title, agent: agentHandle });
  const item = sequence.prompt({ ...fields, id } as never);
  registerItem(`agent.prompt(${JSON.stringify(title)})`, item, title);
  return item;
});

export interface ForEachRegistrarOptions {
  readonly limit?: number;
  readonly maxItems?: number;
  readonly concurrent?: boolean;
  readonly onComplete?: ForEachSequenceFields["onComplete"];
  /**
   * Explicit item-slot schema, overriding the one inherited from the
   * iterated list. Passing an object-schema handle also mints typed field
   * refs on the item slot (`item.foo`), which the inherited ref string
   * cannot.
   */
  readonly itemSchema?: SchemaValue;
}

/**
 * The per-item slot for an `items.forEach` loop. The validator requires the
 * item slot's schema to equal the iterated list's element schema exactly
 * and refuses `schema:any` for either side, so the item slot inherits the
 * element ref straight off the `over` slot's declared `[element]` list
 * schema. An explicit `itemSchema` wins over inheritance. `slot.any` — the
 * pre-0153 mint, which no canonical could ever accept — remains only as the
 * fallback for an untyped or undeclared list, where the validator's own
 * diagnostics name the actual problem.
 */
function forEachItemSlot(over: SlotHandle, loopId: string, explicit: SchemaValue | undefined): SlotHandle {
  const declared = explicit ?? inheritedElementSchema(over);
  const fields = { id: `${loopId}-item`, description: `Per-item value for the ${loopId} loop.` };
  return declared === undefined ? slot.any(fields) : slot({ ...fields, schema: declared });
}

function inheritedElementSchema(over: SlotHandle): SchemaValue | undefined {
  const schemaRef = (metaOf(over)?.declaration as { readonly schema?: unknown } | undefined)?.schema;
  if (typeof schemaRef !== "string" || !schemaRef.startsWith("[") || !schemaRef.endsWith("]")) return undefined;
  // A bare ref string is a valid runtime schema value (`refText` passes it
  // through); only the static type insists on a handle.
  return schemaRef.slice(1, -1) as unknown as SchemaValue;
}

installSlotForEachLowering((slotHandle, titleText, opts, body) => {
  requireBuild(`items.forEach(${JSON.stringify(titleText)})`);
  const id = mintId(titleText);
  const frame = captureAuthorFrame();
  const itemSlot = forEachItemSlot(slotHandle, id, opts?.itemSchema);
  const { scope } = runInScope("for-each", `items.forEach(${JSON.stringify(titleText)})`, () => body(itemSlot));
  checkDuplicateTitles(scope.items, `items.forEach(${JSON.stringify(titleText)})`);
  if (scope.items.length === 0) throw buildError(titleText, "items.forEach registered no steps", frame);
  const forEachOpts = opts ?? {};
  const item = sequence.forEach(
    id,
    withPositionalWhen({
      title: titleText,
      over: slotHandle,
      item: itemSlot,
      body: itemsOf(scope),
      limit: forEachOpts.limit,
      maxItems: forEachOpts.maxItems,
      concurrent: forEachOpts.concurrent,
      onComplete: forEachOpts.onComplete,
    } as Omit<ForEachSequenceFields, "id" | "kind">),
  );
  registerItem(`items.forEach(${JSON.stringify(titleText)})`, item, titleText);
  return item;
});

// ---------------------------------------------------------------------------
// flow.*
// ---------------------------------------------------------------------------

const LOOP_EXHAUSTION_VERBS: Readonly<Partial<Record<SignalVerbName, ExhaustionPolicy>>> = {
  abort: "abort",
  continue: "continue",
};

export interface LoopParam {
  /** Required, callable once — a loop with no way out is not authorable (0102). */
  maxIterations(
    bound: number | SettingHandle<number>,
    opts?: { readonly onExhausted?: SignalVerb<"abort" | "continue"> },
  ): void;
  /** Overrides the loop's emitted canonical id (0109 F2), when it must differ from `idFromTitle(title)`. Callable at most once. */
  id(overrideId: string): void;
  /** Wrapper over `flow.until` — the loop's exit guard, on the param the body already holds. */
  until(cond: BranchCheckValue): void;
  /** Wrapper over `flow.untilAll` — exits once EVERY guard holds. */
  untilAll(guards: readonly GuardValue[]): void;
  /** Wrapper over `flow.untilAny` — exits once ANY guard holds. */
  untilAny(guards: readonly GuardValue[]): void;
}

function combineAbortIfArms(
  arms: readonly { readonly title: string; readonly condition: BranchCheckValue }[],
): BranchCheckValue | undefined {
  if (arms.length === 0) return undefined;
  // oxlint-disable-next-line typescript/no-non-null-assertion -- length === 1 checked above
  if (arms.length === 1) return arms[0]!.condition;
  return condition.any(
    arms.map((arm) => lowerCheckGuard(arm.condition, `flow.when(${JSON.stringify(arm.title)})`).guard),
  );
}

function flowLoop(title: string, body: (loop: LoopParam) => void): SequenceHandle {
  requireBuild(`flow.loop(${JSON.stringify(title)})`);
  const frame = captureAuthorFrame();
  const label = `flow.loop(${JSON.stringify(title)})`;
  const loopParam: LoopParam = {
    maxIterations(bound, opts) {
      const scope = nearestScope("loop");
      if (scope?.loop === undefined) {
        throw buildError(title, "loop.maxIterations(...) called outside its own flow.loop body", frame);
      }
      if (scope.loop.maxIterationsCalled) {
        throw buildError(title, "loop.maxIterations(...) called more than once", frame);
      }
      scope.loop.maxIterationsCalled = true;
      scope.loop.maxIterationsValue = bound;
      if (opts?.onExhausted !== undefined) {
        scope.loop.onExhausted = requireLegalVerb(
          opts.onExhausted,
          LOOP_EXHAUSTION_VERBS,
          "loop.maxIterations({ onExhausted })",
          title,
          frame,
        );
      }
    },
    id(overrideId) {
      const scope = nearestScope("loop");
      if (scope?.loop === undefined) {
        throw buildError(title, "loop.id(...) called outside its own flow.loop body", frame);
      }
      if (scope.loop.idOverride !== undefined) {
        throw buildError(title, "loop.id(...) called more than once", frame);
      }
      scope.loop.idOverride = overrideId;
    },
    until: flowUntil,
    untilAll: flowUntilAll,
    untilAny: flowUntilAny,
  };
  const { scope } = runInScope("loop", label, () => body(loopParam));
  checkDuplicateTitles(scope.items, label);
  // oxlint-disable-next-line typescript/no-non-null-assertion -- runInScope("loop", ...) always populates scope.loop
  const loopState = scope.loop!;
  const id = loopState.idOverride ?? mintId(title);
  if (!loopState.maxIterationsCalled) {
    throw buildError(title, "loop.maxIterations(...) is required — no-way-out is not authorable", frame);
  }
  const abortIf = combineAbortIfArms(loopState.abortIfArms);
  const fields = withPositionalWhen<Omit<LoopSequenceFields, "id" | "kind">>({
    title,
    body: itemsOf(scope),
    ...(loopState.maxIterationsValue === undefined ? {} : { maxIterations: loopState.maxIterationsValue }),
    ...(loopState.untilCondition === undefined ? {} : { until: loopState.untilCondition }),
    ...(abortIf === undefined ? {} : { abortIf }),
    ...(loopState.onExhausted === undefined ? {} : { onExhausted: loopState.onExhausted as ExhaustionPolicy }),
    ...(loopState.onComplete.length === 0 ? {} : { onComplete: loopState.onComplete }),
    ...(loopState.onAbort.length === 0 ? {} : { onAbort: loopState.onAbort }),
  });
  const item = sequence.loop(id, fields);
  registerItem(label, item, title);
  return item;
}

const LOOP_ABORT_ARM_VERBS: Readonly<Partial<Record<SignalVerbName, "abort">>> = { abort: "abort" };

function flowWhen(title: string, cond: BranchCheckValue, arm: SignalVerb<"abort">): void;
function flowWhen(title: string, cond: BranchCheckValue, body: () => void): SequenceHandle;
function flowWhen(title: string, cond: BranchCheckValue, opts: IdOverride, body: () => void): SequenceHandle;
function flowWhen(
  title: string,
  cond: BranchCheckValue,
  thirdArg: SignalVerb | IdOverride | (() => void),
  maybeBody?: () => void,
): SequenceHandle | void {
  requireBuild(`flow.when(${JSON.stringify(title)})`);
  const frame = captureAuthorFrame();
  if (signalVerbOf(thirdArg) !== undefined) {
    requireLegalVerb(thirdArg, LOOP_ABORT_ARM_VERBS, "flow.when(..., <verb>)", title, frame);
    const loopScope = nearestScope("loop");
    if (loopScope?.loop === undefined) {
      throw buildError(title, "flow.when(..., signal.Abort) requires an enclosing flow.loop", frame);
    }
    loopScope.loop.abortIfArms.push({ title, condition: cond });
    return undefined;
  }
  const idOverride = typeof thirdArg === "function" ? undefined : (thirdArg as IdOverride).id;
  // oxlint-disable-next-line typescript/no-non-null-assertion -- the public overloads guarantee maybeBody is supplied whenever thirdArg isn't the body itself
  const body = typeof thirdArg === "function" ? thirdArg : maybeBody!;
  const id = idOverride ?? mintId(title);
  const label = `flow.when(${JSON.stringify(title)})`;
  const { scope } = runInScope("when", label, body);
  checkDuplicateTitles(scope.items, label);
  if (scope.items.length === 0) throw buildError(title, "flow.when block registered no steps", frame);
  // Success-only block: an object-layer `branch` has no `when` of its own, so the positional
  // guard (if any) ANDs into `check` itself instead — see `positionalUntilGuard`.
  const notUntil = positionalUntilGuard();
  const check: BranchCheckValue =
    notUntil === undefined ? cond : condition.all([lowerCheckGuard(cond, label).guard, notUntil]);
  const item = sequence.branch(id, {
    title,
    check,
    success: itemsOf(scope) as [SequenceHandle, ...SequenceHandle[]],
  });
  registerItem(label, item, title);
  return item;
}

function flowUntil(cond: BranchCheckValue): void {
  const scope = currentScope("flow.until(...)");
  if (scope.kind !== "loop" || scope.loop === undefined) {
    throw new Error("flow.until(...) is only valid directly inside a flow.loop body");
  }
  if (scope.loop.untilCondition !== undefined) {
    throw buildError(undefined, "a loop may declare at most one flow.until", captureAuthorFrame());
  }
  scope.loop.untilCondition = cond;
}

type MatchArmCallback = () => void;
export type GuardMatchArms = {
  readonly [CONDITION_TRUE]?: MatchArmCallback;
  readonly [CONDITION_FALSE]?: MatchArmCallback;
};
export type FieldMatchArms = {
  readonly [CONDITION_OTHERWISE]?: MatchArmCallback;
  readonly [key: string]: MatchArmCallback | undefined;
};

function runArmScope(matchTitle: string, armLabel: string, cb: MatchArmCallback): readonly SequenceHandle[] {
  const label = `flow.match(${JSON.stringify(matchTitle)}) arm ${armLabel}`;
  const { scope } = runInScope("match-arm", label, cb);
  checkDuplicateTitles(scope.items, label);
  if (scope.items.length === 0) {
    throw buildError(matchTitle, `flow.match arm ${armLabel} registered no steps`, captureAuthorFrame());
  }
  return itemsOf(scope);
}

function flowMatch(
  title: string,
  subject: BranchCheckValue | SlotHandle | FieldRef,
  arms: GuardMatchArms | FieldMatchArms,
): SequenceHandle {
  requireBuild(`flow.match(${JSON.stringify(title)})`);
  const id = mintId(title);
  const frame = captureAuthorFrame();
  forbidPositionalUntil(title, "flow.match", frame);
  const symbolKeys = Object.getOwnPropertySymbols(arms);
  const isGuardSubject = symbolKeys.includes(CONDITION_TRUE) || symbolKeys.includes(CONDITION_FALSE);
  if (isGuardSubject) {
    const guardArms = arms as GuardMatchArms;
    const trueArm = guardArms[CONDITION_TRUE];
    const falseArm = guardArms[CONDITION_FALSE];
    if (trueArm !== undefined && typeof trueArm !== "function") {
      throw buildError(title, "flow.match arm [condition.True] must be a callback", frame);
    }
    if (falseArm !== undefined && typeof falseArm !== "function") {
      throw buildError(title, "flow.match arm [condition.False] must be a callback", frame);
    }
    if (trueArm === undefined && falseArm === undefined) {
      throw buildError(title, "flow.match requires at least one of [condition.True]/[condition.False]", frame);
    }
    const trueItems = trueArm === undefined ? undefined : runArmScope(title, "[condition.True]", trueArm);
    const falseItems = falseArm === undefined ? undefined : runArmScope(title, "[condition.False]", falseArm);
    const item = sequence.branch(id, {
      title,
      check: subject as BranchCheckValue,
      ...(trueItems === undefined ? {} : { success: trueItems as [SequenceHandle, ...SequenceHandle[]] }),
      ...(falseItems === undefined ? {} : { failure: falseItems as [SequenceHandle, ...SequenceHandle[]] }),
    });
    registerItem(`flow.match(${JSON.stringify(title)})`, item, title);
    return item;
  }

  const fieldArms = arms as FieldMatchArms;
  const otherwiseArm = fieldArms[CONDITION_OTHERWISE];
  const entries = Object.entries(fieldArms) as [string, MatchArmCallback | undefined][];
  for (const [key, value] of entries) {
    if (typeof value !== "function") {
      throw buildError(title, `flow.match arm ${JSON.stringify(key)} must be a callback`, frame);
    }
  }
  if (otherwiseArm !== undefined && typeof otherwiseArm !== "function") {
    throw buildError(title, "flow.match arm [condition.Otherwise] must be a callback", frame);
  }
  if (entries.length === 0) {
    throw buildError(title, "flow.match requires at least one value arm", frame);
  }
  const otherwiseItems =
    otherwiseArm === undefined ? undefined : runArmScope(title, "[condition.Otherwise]", otherwiseArm);
  let nested: SequenceHandle | undefined;
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    // oxlint-disable-next-line typescript/no-non-null-assertion -- index stays within [0, entries.length)
    const [value, cb] = entries[index]!;
    // oxlint-disable-next-line typescript/no-non-null-assertion -- every value in entries was checked to be a function above
    const armItems = runArmScope(title, JSON.stringify(value), cb!);
    const branchId = index === 0 ? id : `${id}-${idFromTitle(value)}`;
    const failureArm = nested === undefined ? otherwiseItems : [nested];
    nested = sequence.branch(branchId, {
      title: index === 0 ? title : `${title} — ${value}`,
      check: condition.equals(subject as SlotHandle<JsonValue> | FieldRef<JsonValue>, value as JsonValue),
      success: armItems as [SequenceHandle, ...SequenceHandle[]],
      ...(failureArm === undefined ? {} : { failure: failureArm as [SequenceHandle, ...SequenceHandle[]] }),
    });
  }
  // oxlint-disable-next-line typescript/no-non-null-assertion -- entries.length === 0 threw above, so the loop ran at least once and assigned nested
  registerItem(`flow.match(${JSON.stringify(title)})`, nested!, title);
  // oxlint-disable-next-line typescript/no-non-null-assertion -- see above
  return nested!;
}

export interface ParParam {
  /** Reserved surface (0102 ledger): accepts the call, emits nothing, and always throws. */
  maxAtOnce(n: number): void;
}

function branchRefFor(loopId: string, registered: RegisteredItem): SequenceLinearHandle {
  const stepHandle = registered.item as SequenceHandle & { readonly id?: string };
  const branchKey = typeof stepHandle.id === "string" ? stepHandle.id : idFromTitle(registered.title ?? "branch");
  return sequence.linear(`${loopId}-branch-${branchKey}`, [stepHandle]);
}

function flowParallel(title: string, body: (par: ParParam) => void): SequenceHandle {
  requireBuild(`flow.parallel(${JSON.stringify(title)})`);
  const id = mintId(title);
  const frame = captureAuthorFrame();
  forbidPositionalUntil(title, "flow.parallel", frame);
  const label = `flow.parallel(${JSON.stringify(title)})`;
  const parParam: ParParam = {
    maxAtOnce() {
      throw new Error("par.maxAtOnce is parked — see the 0102 ledger");
    },
  };
  const { scope } = runInScope("parallel", label, () => body(parParam));
  checkDuplicateTitles(scope.items, label);
  if (scope.items.length === 0) throw buildError(title, "flow.parallel registered no branches", frame);
  const branches = scope.items.map((registered) => branchRefFor(id, registered));
  const policy = scope.parallel?.onFailurePolicy as ParallelBranchFailurePolicy | undefined;
  const announce = scope.parallel?.onFailureAnnounce;
  const options: ParallelOptions = {
    ...(policy === undefined ? {} : { branchFailure: branches.map((branch) => ({ branch, onFailure: policy })) }),
    ...(announce === undefined ? {} : { onFailure: announce }),
  };
  const item = sequence.parallel(id, branches, options);
  registerItem(label, item, title);
  return item;
}

/** Sugar over `flow.until(condition.all([...]))` — the loop exits once EVERY guard holds. */
function flowUntilAll(guards: readonly GuardValue[]): void {
  flowUntil(condition.all(guards));
}

/** Sugar over `flow.until(condition.any([...]))` — the loop exits once ANY guard holds. */
function flowUntilAny(guards: readonly GuardValue[]): void {
  flowUntil(condition.any(guards));
}

/** Payload value for an authored terminal exit: a local slot handle or `operation.literal(...)`. */
type TerminalPayloadValue = TerminalBinding["source"];

/** Reserved output-port id the error terminal's typed record lands on (mirrors the Rust `TERMINAL_ERROR_PORT_ID`). */
const FLOW_ERROR_PORT_ID = "flow-error";

export interface FlowErrorOptions extends IdOverride {
  /** Report headline; defaults to the title. */
  readonly message?: string;
  /** Evidence written to the reserved `flow-error` output port as the exit's typed error record. */
  readonly evidence?: TerminalPayloadValue;
}

export interface FlowSuccessOptions extends IdOverride {
  /** Report headline; defaults to the title. */
  readonly message?: string;
  /** Exit bindings: declared output-port id -> slot handle or `operation.literal(...)`. */
  readonly bind?: Readonly<Record<string, TerminalPayloadValue>>;
}

/**
 * Authored ERROR terminal (0189): the run ends HERE, failure-shaped — the
 * message becomes the report headline and stop reason, the evidence (if
 * given) the typed record on the reserved `flow-error` output port. Inside a
 * `flow.loop` this still ends the RUN, never just the loop — loop-scoped
 * aborts remain `signal.Abort`.
 */
function flowError(title: string, opts: FlowErrorOptions = {}): SequenceHandle {
  requireBuild(`flow.error(${JSON.stringify(title)})`);
  const frame = captureAuthorFrame();
  forbidPositionalUntil(title, "flow.error", frame);
  const id = opts.id ?? mintId(title);
  const item = sequence.terminal(id, {
    title,
    outcome: "error",
    message: opts.message ?? title,
    payload: opts.evidence === undefined ? [] : [{ destination: FLOW_ERROR_PORT_ID, source: opts.evidence }],
  });
  registerItem(`flow.error(${JSON.stringify(title)})`, item, title);
  return item;
}

/**
 * Authored SUCCESS terminal (0189): the run completes HERE, binding declared
 * output ports through `bind`. Declaring any success terminal opts the trait
 * into exit-point completion: falling through every exit becomes an authored
 * `no-exit-reached` failure instead of quiet completion.
 */
function flowSuccess(title: string, opts: FlowSuccessOptions = {}): SequenceHandle {
  requireBuild(`flow.success(${JSON.stringify(title)})`);
  const frame = captureAuthorFrame();
  forbidPositionalUntil(title, "flow.success", frame);
  const id = opts.id ?? mintId(title);
  const item = sequence.terminal(id, {
    title,
    outcome: "success",
    message: opts.message ?? title,
    payload: Object.entries(opts.bind ?? {}).map(([destination, source]) => ({ destination, source })),
  });
  registerItem(`flow.success(${JSON.stringify(title)})`, item, title);
  return item;
}

/** Fused `when` + error terminal — lowers to exactly flow.when(title, condition, () => flow.error(...)), never a separate canonical form. */
function flowErrorWhen(title: string, cond: BranchCheckValue, opts: FlowErrorOptions = {}): SequenceHandle {
  return flowWhen(title, cond, () => {
    flowError(title, { ...opts, id: opts.id ?? `${idFromTitle(title)}-exit` });
  });
}

/** Fused `when` + success terminal — lowers to exactly flow.when(title, condition, () => flow.success(...)), never a separate canonical form. */
function flowSuccessWhen(title: string, cond: BranchCheckValue, opts: FlowSuccessOptions = {}): SequenceHandle {
  return flowWhen(title, cond, () => {
    flowSuccess(title, { ...opts, id: opts.id ?? `${idFromTitle(title)}-exit` });
  });
}

export const flow = {
  loop: flowLoop,
  when: flowWhen,
  error: flowError,
  success: flowSuccess,
  errorWhen: flowErrorWhen,
  successWhen: flowSuccessWhen,
  until: flowUntil,
  untilAll: flowUntilAll,
  untilAny: flowUntilAny,
  match: flowMatch,
  parallel: flowParallel,
};

// ---------------------------------------------------------------------------
// effect.*
// ---------------------------------------------------------------------------

const PARALLEL_FAILURE_VERBS: Readonly<Partial<Record<SignalVerbName, ParallelBranchFailurePolicy>>> = {
  skip: "skip",
  park: "park",
  abort: "panel-fail",
};

export const effect = {
  onComplete(signal: SignalOutputValue): void {
    const scope = nearestScope("loop");
    if (scope?.loop === undefined) throw new Error("effect.onComplete(...) requires an enclosing flow.loop");
    scope.loop.onComplete.push(signal);
  },
  /**
   * Attaches to the nearest enclosing `flow.parallel` (0209): a declared
   * signal (string or `RefHandle<"signal">`) announces the panel-level
   * failure recovery route, once; a verb (`signal.Skip`/`signal.Park`/
   * `signal.Abort` — `signal.Continue` is not a branch-failure outcome)
   * decides every branch's failure policy, once. Both may be called in the
   * same `flow.parallel` body, in either order.
   */
  onFailure(target: SignalVerb<"abort" | "skip" | "park"> | string | RefHandle<"signal">): void {
    const scope = nearestScope("parallel");
    if (scope?.parallel === undefined) {
      throw new Error(
        "effect.onFailure(...) has no target here — a flow.loop declares no failure of its own to route (0102)",
      );
    }
    if (signalVerbOf(target) === undefined) {
      if (scope.parallel.onFailureAnnounce !== undefined) {
        throw new Error("effect.onFailure(...) announce already declared once — canonical on-failure is single-valued");
      }
      scope.parallel.onFailureAnnounce = target as string | RefHandle<"signal">;
      return;
    }
    if (scope.parallel.onFailurePolicy !== undefined) {
      throw new Error("effect.onFailure(...) decision verb already declared once");
    }
    const { verbName, mapped, allowed } = mapLegalVerb(target, PARALLEL_FAILURE_VERBS);
    if (mapped === undefined) {
      // `verbName` is defined here — the caller already routed non-verb `target` values to the announce branch above.
      throw new Error(
        `effect.onFailure(${signalVerbLabel(verbName as SignalVerbName)}) is not legal here — allowed: ${allowed}`,
      );
    }
    scope.parallel.onFailurePolicy = mapped;
  },
  onAbort(signal: ExhaustionSignalValue): void {
    const scope = nearestScope("loop");
    if (scope?.loop === undefined) throw new Error("effect.onAbort(...) requires an enclosing flow.loop");
    scope.loop.onAbort.push(signal);
  },
  /**
   * `[sink.session-title]` (task 0110): session-global, position-free (may
   * be called anywhere in a `procedure.from(...)`/trait-function body, not
   * just its root scope) — unlike `effect.on*`, this never attaches to
   * `nearestScope`. Mode is inferred from the shape of `X`: a `string` or
   * `input.prompt` template is verbatim; a bare slot or array of slots is
   * generated. A second declaration in the same build is a build error (the
   * `use*` overlap rule).
   * @example `effect.session.title(input.prompt\`Working on ${topic}\`)`
   * @example `effect.session.title(draft)`
   */
  session: {
    title(input: SessionTitleSinkInput): void {
      declareSessionTitleSink(input);
    },
  },
};
