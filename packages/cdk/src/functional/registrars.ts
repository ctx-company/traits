/**
 * The functional registration DSL (0106): `step.*`, `flow.*`, `effect.*`,
 * plus the lowering installed into `context.ts` for `agent.prompt`/
 * `slot.forEach`. Every emission here goes through an existing object-layer
 * constructor (`sequence.*`) — this module never builds canonical JSON by
 * hand (0106 Watch: "the functional layer may not reach around the object
 * layer").
 */
import type { BranchCheckValue, GuardValue } from "../condition.js";
import { condition, lowerCheckGuard } from "../condition.js";
import type { JsonValue } from "../generated.js";
import type { FieldRef, SequenceHandle, SequenceLinearHandle, SettingHandle, SlotHandle } from "../handles.js";
import type {
  CheckSequenceFields,
  CommandSequenceFields,
  ExhaustionPolicy,
  ExhaustionSignalValue,
  ForEachSequenceFields,
  LoopSequenceFields,
  ParallelBranchFailurePolicy,
  ProjectSequenceFields,
  SignalOutputValue,
} from "../sequence.js";
import { idFromTitle, sequence, validateNoDuplicateTitles } from "../sequence.js";
import type { SessionTitleSinkInput } from "../sink.js";
import { slot } from "../slot.js";
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
  ): SequenceHandle & { readonly pass: SlotHandle<boolean> } {
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
}

installSlotForEachLowering((slotHandle, titleText, opts, body) => {
  requireBuild(`items.forEach(${JSON.stringify(titleText)})`);
  const id = mintId(titleText);
  const frame = captureAuthorFrame();
  const itemSlot = slot.any({ id: `${id}-item`, description: `Per-item value for the ${id} loop.` });
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

export interface LoopParam {
  /** Required, callable once — a loop with no way out is not authorable (0102). */
  maxIterations(bound: number | SettingHandle<number>, opts?: { readonly onExhausted?: ExhaustionPolicy }): void;
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
      if (opts?.onExhausted !== undefined) scope.loop.onExhausted = opts.onExhausted;
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

const FLOW_ABORT_ARM = "abort" as const;

function flowWhen(title: string, cond: BranchCheckValue, arm: typeof FLOW_ABORT_ARM): void;
function flowWhen(title: string, cond: BranchCheckValue, body: () => void): SequenceHandle;
function flowWhen(title: string, cond: BranchCheckValue, opts: IdOverride, body: () => void): SequenceHandle;
function flowWhen(
  title: string,
  cond: BranchCheckValue,
  thirdArg: typeof FLOW_ABORT_ARM | IdOverride | (() => void),
  maybeBody?: () => void,
): SequenceHandle | void {
  requireBuild(`flow.when(${JSON.stringify(title)})`);
  const frame = captureAuthorFrame();
  if (thirdArg === FLOW_ABORT_ARM) {
    const loopScope = nearestScope("loop");
    if (loopScope?.loop === undefined) {
      throw buildError(title, "flow.when(..., flow.Abort) requires an enclosing flow.loop", frame);
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

const FLOW_TRUE: unique symbol = Symbol("flow.True");
const FLOW_FALSE: unique symbol = Symbol("flow.False");
const FLOW_OTHERWISE: unique symbol = Symbol("flow.Otherwise");

type MatchArmCallback = () => void;
export type GuardMatchArms = { readonly [FLOW_TRUE]?: MatchArmCallback; readonly [FLOW_FALSE]?: MatchArmCallback };
export type FieldMatchArms = {
  readonly [FLOW_OTHERWISE]?: MatchArmCallback;
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
  const isGuardSubject = symbolKeys.includes(FLOW_TRUE) || symbolKeys.includes(FLOW_FALSE);
  if (isGuardSubject) {
    const guardArms = arms as GuardMatchArms;
    const trueArm = guardArms[FLOW_TRUE];
    const falseArm = guardArms[FLOW_FALSE];
    if (trueArm !== undefined && typeof trueArm !== "function") {
      throw buildError(title, "flow.match arm [flow.True] must be a callback", frame);
    }
    if (falseArm !== undefined && typeof falseArm !== "function") {
      throw buildError(title, "flow.match arm [flow.False] must be a callback", frame);
    }
    if (trueArm === undefined && falseArm === undefined) {
      throw buildError(title, "flow.match requires at least one of [flow.True]/[flow.False]", frame);
    }
    const trueItems = trueArm === undefined ? undefined : runArmScope(title, "[flow.True]", trueArm);
    const falseItems = falseArm === undefined ? undefined : runArmScope(title, "[flow.False]", falseArm);
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
  const otherwiseArm = fieldArms[FLOW_OTHERWISE];
  const entries = Object.entries(fieldArms) as [string, MatchArmCallback | undefined][];
  for (const [key, value] of entries) {
    if (typeof value !== "function") {
      throw buildError(title, `flow.match arm ${JSON.stringify(key)} must be a callback`, frame);
    }
  }
  if (otherwiseArm !== undefined && typeof otherwiseArm !== "function") {
    throw buildError(title, "flow.match arm [flow.Otherwise] must be a callback", frame);
  }
  if (entries.length === 0) {
    throw buildError(title, "flow.match requires at least one value arm", frame);
  }
  const otherwiseItems = otherwiseArm === undefined ? undefined : runArmScope(title, "[flow.Otherwise]", otherwiseArm);
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
  /** Sets the branch-failure policy applied to every branch registered in this `flow.parallel` block. */
  onFailure(policy: ParallelBranchFailurePolicy): void;
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
    onFailure(policy) {
      const scope = nearestScope("parallel");
      if (scope?.parallel === undefined) {
        throw buildError(title, "par.onFailure(...) called outside its own flow.parallel body", frame);
      }
      scope.parallel.onFailurePolicy = policy;
    },
    maxAtOnce() {
      throw new Error("par.maxAtOnce is parked — see the 0102 ledger");
    },
  };
  const { scope } = runInScope("parallel", label, () => body(parParam));
  checkDuplicateTitles(scope.items, label);
  if (scope.items.length === 0) throw buildError(title, "flow.parallel registered no branches", frame);
  const branches = scope.items.map((registered) => branchRefFor(id, registered));
  const policy = scope.parallel?.onFailurePolicy as ParallelBranchFailurePolicy | undefined;
  const item = sequence.parallel(
    id,
    branches,
    policy === undefined ? {} : { branchFailure: branches.map((branch) => ({ branch, onFailure: policy })) },
  );
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

export const flow = {
  loop: flowLoop,
  when: flowWhen,
  until: flowUntil,
  untilAll: flowUntilAll,
  untilAny: flowUntilAny,
  match: flowMatch,
  parallel: flowParallel,
  True: FLOW_TRUE,
  False: FLOW_FALSE,
  Otherwise: FLOW_OTHERWISE,
  Abort: FLOW_ABORT_ARM,
  Continue: "continue" as const,
};

// ---------------------------------------------------------------------------
// effect.*
// ---------------------------------------------------------------------------

export const effect = {
  onComplete(signal: SignalOutputValue): void {
    const scope = nearestScope("loop");
    if (scope?.loop === undefined) throw new Error("effect.onComplete(...) requires an enclosing flow.loop");
    scope.loop.onComplete.push(signal);
  },
  onFailure(): void {
    throw new Error(
      "effect.onFailure(...) has no target here — a flow.loop declares no failure of its own to route (0102)",
    );
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
