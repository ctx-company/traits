/**
 * The synchronous build-context stack for the functional authoring layer
 * (0106). Zero runtime dependency on `sequence.ts` — see
 * `.internal/tasks/0106-functional-procedures-inside-object-shells.md`
 * ("Risks: import cycles"): `agent.ts`/`slot.ts` must reach this module to
 * implement their non-enumerable `.prompt`/`.forEach` registrars without
 * themselves importing `sequence.ts` (which already imports `slot.ts`).
 * `registrars.ts` is the only module that reads/writes scope state directly
 * and also imports `sequence.ts`; `agent.ts`/`slot.ts` only ever call the
 * generic dispatch functions below, installed by `registrars.ts` at module
 * init.
 */

/** Best-effort authoring location captured at a registrar call site. */
export interface AuthorFrame {
  readonly file: string;
}

const CONTEXT_SOURCE_PATH = decodeURIComponent(new URL(import.meta.url).pathname);

/** Walks the call stack to the first frame outside `packages/cdk/src` — mirrors `meta.ts`'s `captureSourceAnchor`, kept local so this module stays import-free of `sequence.ts`/`meta.ts`. */
export function captureAuthorFrame(): AuthorFrame | undefined {
  const stack = new Error().stack;
  if (stack === undefined) return undefined;
  for (const line of stack.split("\n").slice(1)) {
    const match = /(?:at\s+(?:.*?\()?)(file:\/\/[^:)]+|\/?[^:)]+):(\d+):(\d+)\)?$/.exec(line.trim());
    if (match === null) continue;
    const rawFile = match[1] ?? "";
    const file = rawFile.startsWith("file://") ? decodeURIComponent(new URL(rawFile).pathname) : rawFile;
    if (file === CONTEXT_SOURCE_PATH || file.includes("/packages/cdk/src/") || file.includes("/packages/cdk/dist/")) {
      continue;
    }
    return { file };
  }
  return undefined;
}

/** Formats a build-rule failure per 0106: `<file>: step titled "X" — <rule>`. `file` is omitted when the author frame could not be resolved. */
export function buildError(title: string | undefined, rule: string, frame?: AuthorFrame): Error {
  const subject = title === undefined ? "(untitled)" : JSON.stringify(title);
  const prefix = frame === undefined ? "" : `${frame.file}: `;
  return new Error(`${prefix}step titled ${subject} — ${rule}`);
}

export type ScopeKind = "root" | "loop" | "when" | "match-arm" | "parallel" | "for-each";

/** One item registered functionally, in registration order. Opaque to `context.ts` — never inspected beyond pass-through. */
export interface RegisteredItem {
  readonly item: unknown;
  readonly title: string | undefined;
}

export type LoopExitPolicy = "continue" | "abort";

export interface LoopScopeState {
  /** Once set, everything registered into this loop scope from this point onward — leaf or container — is guarded `not(cond)` (0106 positional lowering). */
  untilCondition?: unknown;
  readonly abortIfArms: { readonly title: string; readonly condition: unknown; }[];
  maxIterationsCalled: boolean;
  maxIterationsValue?: number;
  onExhausted?: LoopExitPolicy;
  readonly onComplete: unknown[];
  readonly onAbort: unknown[];
}

export interface ParallelScopeState {
  onFailurePolicy?: unknown;
}

export interface Scope {
  readonly kind: ScopeKind;
  readonly items: RegisteredItem[];
  readonly loop?: LoopScopeState;
  readonly parallel?: ParallelScopeState;
}

interface BuildFrame {
  readonly scopes: Scope[];
}

let activeBuild: BuildFrame | undefined;

export function isBuildActive(): boolean {
  return activeBuild !== undefined;
}

export function openBuild(): void {
  if (activeBuild !== undefined) {
    throw new Error("procedure.from: a functional build is already in progress — one build at a time");
  }
  activeBuild = { scopes: [{ kind: "root", items: [] }] };
}

/** Closes the root scope and returns its registered items — throws if scopes are unbalanced (a `flow.*` block never popped its scope). */
export function closeBuild(): readonly RegisteredItem[] {
  const build = activeBuild;
  if (build === undefined) throw new Error("procedure.from: no active build to close");
  if (build.scopes.length !== 1) {
    throw new Error("procedure.from: build closed with unbalanced open scopes — a flow.* block never returned");
  }
  const [root] = build.scopes;
  activeBuild = undefined;
  return root!.items;
}

function requireBuild(caller: string): BuildFrame {
  if (activeBuild === undefined) {
    throw new Error(`${caller} called outside procedure.from`);
  }
  return activeBuild;
}

export function currentScope(caller: string): Scope {
  const scope = requireBuild(caller).scopes.at(-1);
  if (scope === undefined) throw new Error(`${caller}: no active scope`);
  return scope;
}

/** The nearest enclosing scope of `kind`, or `undefined` if none is open. */
export function nearestScope(kind: ScopeKind): Scope | undefined {
  if (activeBuild === undefined) return undefined;
  for (let index = activeBuild.scopes.length - 1; index >= 0; index -= 1) {
    const scope = activeBuild.scopes[index];
    if (scope !== undefined && scope.kind === kind) return scope;
  }
  return undefined;
}

function pushScope(kind: ScopeKind, caller: string): Scope {
  const build = requireBuild(caller);
  const scope: Scope = {
    kind,
    items: [],
    ...(kind === "loop"
      ? { loop: { abortIfArms: [], maxIterationsCalled: false, onComplete: [], onAbort: [] } }
      : {}),
    ...(kind === "parallel" ? { parallel: {} } : {}),
  };
  build.scopes.push(scope);
  return scope;
}

function popScope(caller: string): Scope {
  const build = requireBuild(caller);
  const scope = build.scopes.pop();
  if (scope === undefined) throw new Error(`${caller}: no open scope to close`);
  return scope;
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return typeof value === "object" && value !== null && typeof (value as { then?: unknown; }).then === "function";
}

/**
 * Runs `callback` inside a new scope, synchronously. Pops the scope in
 * `finally` regardless of how `callback` returns; a callback that returned a
 * thenable is the async-block build error (checked only after the scope is
 * safely popped, so the stack never wedges open on that path).
 */
export function runInScope<T>(
  kind: ScopeKind,
  label: string,
  callback: () => T,
): { readonly result: T; readonly scope: Scope; } {
  pushScope(kind, label);
  let result: T;
  let scope: Scope;
  try {
    result = callback();
  } finally {
    scope = popScope(label);
  }
  if (isThenable(result)) {
    throw new Error(`${label}: async callbacks are not supported — flow.* bodies must run synchronously`);
  }
  return { result, scope };
}

export function registerItem(caller: string, item: unknown, title?: string): void {
  currentScope(caller).items.push({ item, title });
}

/**
 * Indirection that lets `agent.ts`/`slot.ts` attach `.prompt`/`.forEach`
 * registrars at handle mint time without importing `sequence.ts`.
 * `registrars.ts` installs the real lowering once, at module init; importing
 * `@ctx-traits/cdk` (which re-exports the functional entry) is what
 * guarantees the installation has run before authoring code can call
 * `.prompt`/`.forEach` on a handle.
 */
type AgentPromptLowering = (agentHandle: unknown, title: string, opts: unknown) => unknown;
type SlotForEachLowering = (slotHandle: unknown, title: string, opts: unknown, body: unknown) => unknown;

let agentPromptLowering: AgentPromptLowering | undefined;
let slotForEachLowering: SlotForEachLowering | undefined;

export function installAgentPromptLowering(fn: AgentPromptLowering): void {
  agentPromptLowering = fn;
}

export function installSlotForEachLowering(fn: SlotForEachLowering): void {
  slotForEachLowering = fn;
}

export function dispatchAgentPrompt(agentHandle: unknown, title: string, opts: unknown): unknown {
  if (agentPromptLowering === undefined) {
    throw new Error(
      "agent.prompt: the functional layer has not loaded — import \"@ctx-traits/cdk\" before calling .prompt(...)",
    );
  }
  return agentPromptLowering(agentHandle, title, opts);
}

export function dispatchSlotForEach(slotHandle: unknown, title: string, opts: unknown, body: unknown): unknown {
  if (slotForEachLowering === undefined) {
    throw new Error(
      "slot.forEach: the functional layer has not loaded — import \"@ctx-traits/cdk\" before calling .forEach(...)",
    );
  }
  return slotForEachLowering(slotHandle, title, opts, body);
}

// ---------------------------------------------------------------------------
// Trait-level frame (0107): `defineTrait`/`use*`/`ctx.input` state for a
// full-file functional trait entry. Distinct from the scope stack above — a
// trait function opens exactly one `BuildFrame` (for its own root-scope
// steps, via `openBuild`/`closeBuild`) and exactly one `TraitFrame` (for
// `defineTrait`'s payload and the `use*` gates), at the same time, for the
// same call to `evaluateTraitFunction`.
// ---------------------------------------------------------------------------

/**
 * One resource/signal/port/slot declared while a trait frame is active —
 * recorded by `port.ts`/`slot.ts`/`procedure.ts`'s builders via
 * `recordTraitMint`, so `functional/trait.ts`'s never-referenced check and
 * `ctx.input` validation see exactly the declarations minted by THIS trait
 * function's own run, not every declaration ever minted in the process (a
 * process-global registry would leak unrelated declarations from other
 * trait builds sharing the same process, e.g. in a test suite).
 */
export interface FrameMint {
  readonly kind: "resource" | "signal" | "port" | "slot";
  readonly id: string;
  readonly ref: string;
  readonly declaration: unknown;
  readonly file: string | undefined;
}

/** Accumulated state for one `evaluateTraitFunction` call. Opaque to `context.ts` beyond its own bookkeeping — `functional/trait.ts` reads/writes the field contents. */
export interface TraitFrame {
  defineTraitCalled: boolean;
  slug: string | undefined;
  /** `defineTrait`'s own fields payload, validated but otherwise untyped here — see `functional/trait.ts`'s `DefineTraitFields`. */
  fields: Record<string, unknown> | undefined;
  /** Accumulated across every `useBehavior` call, keyed by `BehaviorFields` field name. */
  readonly behavior: Record<string, unknown>;
  /** Accumulated across every `useIntent` call, keyed by `IntentSpec` facet name. */
  readonly intent: Record<string, unknown>;
  /** Resource handles passed to `useResource`, in call order. */
  readonly resources: unknown[];
  /** Every `ctx.input.*` id (kebab-cased) accessed during the trait function's run. */
  readonly inputAccessed: Set<string>;
  /** Every resource/signal/port/slot minted while this frame was active. @see {@link FrameMint} */
  readonly mints: FrameMint[];
}

let activeTraitFrame: TraitFrame | undefined;

export function isTraitFrameActive(): boolean {
  return activeTraitFrame !== undefined;
}

export function openTraitFrame(): void {
  if (activeTraitFrame !== undefined) {
    throw new Error("evaluateTraitFunction: a trait function is already being evaluated — one at a time");
  }
  activeTraitFrame = {
    defineTraitCalled: false,
    slug: undefined,
    fields: undefined,
    behavior: {},
    intent: {},
    resources: [],
    inputAccessed: new Set(),
    mints: [],
  };
}

/** Closes and returns the active trait frame — throws if none is open. */
export function closeTraitFrame(): TraitFrame {
  const frame = activeTraitFrame;
  if (frame === undefined) throw new Error("evaluateTraitFunction: no active trait function to close");
  activeTraitFrame = undefined;
  return frame;
}

/** The active trait frame, or a build error naming `caller` if none is open — every `defineTrait`/`use*` gate calls this first. */
export function currentTraitFrame(caller: string): TraitFrame {
  if (activeTraitFrame === undefined) {
    throw new Error(`${caller} called outside a trait function — it must run inside evaluateTraitFunction's callback`);
  }
  return activeTraitFrame;
}

/**
 * Records one minted resource/signal/port/slot declaration into the active
 * trait frame — a no-op when no trait frame is active, so ordinary
 * object-layer authoring (outside `evaluateTraitFunction`) pays no cost and
 * leaves no trace. Called by `port.ts`/`slot.ts`/`procedure.ts`'s own
 * builders right after they mint a declaration.
 */
export function recordTraitMint(kind: FrameMint["kind"], id: string, ref: string, declaration: unknown): void {
  if (activeTraitFrame === undefined) return;
  activeTraitFrame.mints.push({ kind, id, ref, declaration, file: captureAuthorFrame()?.file });
}
