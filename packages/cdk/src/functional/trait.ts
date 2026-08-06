/**
 * The full-file functional trait entry (0107): `defineTrait`, the `use*`
 * outcome gates, `ctx.input`, and `evaluateTraitFunction` — the harness call
 * for a `export default function (ctx) { ... }` trait module. Everything
 * here lowers through the existing object-layer `trait()`/
 * `assembleSingleTraitDraft` path (`../trait.js`) — no second emitter (0102
 * Watch).
 */
import type { PortHandle, ResourceHandle, SequenceHandle, SlotHandle } from "../handles.js";
import type { MetaDeclaration } from "../meta.js";
import { metaOf, withMeta } from "../meta.js";
import { isSlug, toDraftJsonWithSourceMap } from "../normalize.js";
import { port } from "../port.js";
import { procedure as procedureOf } from "../procedure.js";
import { ref } from "../ref.js";
import type { BehaviorFields, IntentSpec, SemVer, TraitFields, TraitMetadata } from "../trait.js";
import { trait } from "../trait.js";
import type { FrameMint, RegisteredItem, TraitFrame } from "./context.js";
import { closeBuild, closeTraitFrame, currentTraitFrame, openBuild, openTraitFrame } from "./context.js";

/** `defineTrait`'s own fields: identity and metadata only — everything else is derived from `use*` calls, `ctx.input`, steps, and the return statement. */
export interface DefineTraitFields {
  readonly name?: string;
  readonly version?: SemVer;
  readonly summary?: string;
  readonly metadata?: TraitMetadata;
  /**
   * The procedure's description text. Required exactly when the trait
   * function registers at least one step (`step.*`/`agent.prompt`/`flow.*`)
   * — a behavioral trait (no steps) never reads this.
   */
  readonly procedure?: string;
}

const DEFINE_TRAIT_KEYS: readonly string[] = ["name", "version", "summary", "metadata", "procedure"];
const BEHAVIOR_KEYS: readonly string[] = [
  "tone",
  "method",
  "verbosity",
  "directness",
  "scopeControl",
  "initiative",
  "uncertainty",
  "format",
];
const INTENT_KEYS: readonly string[] = ["require", "focus", "avoid", "block"];

function assertKnownKeys(fields: Record<string, unknown>, allowed: readonly string[], caller: string): void {
  const unknown = Object.keys(fields).filter((key) => !allowed.includes(key));
  if (unknown.length > 0) {
    throw new Error(`${caller}: unknown field(s) ${unknown.join(", ")} — expected one of ${allowed.join(", ")}`);
  }
}

/** Catches the enum-typo case (e.g. `intent.avoid.RubberStampReviw`, which evaluates to `undefined`) as an array entry instead of a silently dropped guidance item. */
function assertNoUndefinedArrayEntries(fields: Record<string, unknown>, caller: string): void {
  for (const [key, value] of Object.entries(fields)) {
    if (Array.isArray(value)) {
      const index = value.findIndex((item) => item === undefined);
      if (index !== -1) {
        throw new Error(`${caller}: ${key}[${index}] is undefined — check for a typo'd built-in guidance reference`);
      }
    }
  }
}

/**
 * `defineTrait`'s literalness check: plain JSON data only (strings, numbers,
 * booleans, arrays, plain objects) — no functions, symbols, or CDK handles.
 * Runtime cannot prove the *source expression* was a literal (that guarantee
 * is the Rust-side text scan over the `defineTrait(...)` call plus the
 * future sandboxed supply-chain lift); this only proves the resulting value
 * is JSON-safe.
 */
function assertJsonLiteral(value: unknown, path: string): void {
  if (value === undefined || value === null) return;
  const kind = typeof value;
  if (kind === "string" || kind === "number" || kind === "boolean") return;
  if (kind === "function" || kind === "symbol") {
    throw new Error(`defineTrait: ${path} must be plain literal data — got a ${kind}`);
  }
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertJsonLiteral(item, `${path}[${index}]`));
    return;
  }
  if (kind === "object") {
    if (metaOf(value) !== undefined) {
      throw new Error(`defineTrait: ${path} must be plain literal data — got a CDK handle`);
    }
    for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
      assertJsonLiteral(item, `${path}.${key}`);
    }
    return;
  }
  throw new Error(`defineTrait: ${path} has an unsupported value type`);
}

/**
 * Declares this trait function's identity — exactly once, plain literal
 * data only. Missing, duplicate, or computed identity is a build error.
 * @example `defineTrait("code-review", { version: "0.1.0", summary: "Reviews a diff." })`
 */
export function defineTrait(slug: string, fields: DefineTraitFields = {}): void {
  const frame = currentTraitFrame("defineTrait");
  if (frame.defineTraitCalled) {
    throw new Error("defineTrait: called more than once — a trait function declares its identity exactly once");
  }
  if (!isSlug(slug)) {
    throw new Error(`defineTrait: expected a lowercase slug, got ${JSON.stringify(slug)}`);
  }
  assertKnownKeys(fields as Record<string, unknown>, DEFINE_TRAIT_KEYS, "defineTrait");
  assertJsonLiteral(fields, "defineTrait fields");
  frame.defineTraitCalled = true;
  frame.slug = slug;
  frame.fields = { ...fields };
}

function guidanceSlug(value: unknown): string | undefined {
  if (typeof value === "string") return value;
  if (value !== null && typeof value === "object" && typeof (value as { id?: unknown; }).id === "string") {
    return (value as { id: string; }).id;
  }
  return undefined;
}

function guidanceSlugs(value: unknown): string[] {
  if (value === undefined) return [];
  const list = Array.isArray(value) ? value : [value];
  return list.map(guidanceSlug).filter((slug): slug is string => slug !== undefined);
}

/**
 * Composes into this trait's `[behavior]` facets. Spread composition per
 * 0102: the builder validates only the final object's shape — unknown
 * keys, `undefined` array entries, and a facet already set by an earlier
 * `useBehavior` call in this trait function.
 * @example `useBehavior({ tone: tone.Direct })`
 */
export function useBehavior(fields: BehaviorFields): void {
  const frame = currentTraitFrame("useBehavior");
  const record = fields as Record<string, unknown>;
  assertKnownKeys(record, BEHAVIOR_KEYS, "useBehavior");
  assertNoUndefinedArrayEntries(record, "useBehavior");
  for (const key of Object.keys(record)) {
    if (key in frame.behavior) {
      throw new Error(`useBehavior: "${key}" was already set by an earlier useBehavior call in this trait function`);
    }
  }
  Object.assign(frame.behavior, record);
}

/**
 * Composes into this trait's `[intent]` facets. Same outcome-only
 * validation as `useBehavior`, plus a require/avoid contradiction check: the
 * same guidance slug declared in both `require` and `avoid` (across any
 * combination of calls) is a build error.
 * @example `useIntent({ require: [intent.require.CiteEvidence] })`
 */
export function useIntent(fields: IntentSpec): void {
  const frame = currentTraitFrame("useIntent");
  const record = fields as Record<string, unknown>;
  assertKnownKeys(record, INTENT_KEYS, "useIntent");
  assertNoUndefinedArrayEntries(record, "useIntent");
  for (const key of Object.keys(record)) {
    if (key in frame.intent) {
      throw new Error(`useIntent: "${key}" was already set by an earlier useIntent call in this trait function`);
    }
  }
  const merged = { ...frame.intent, ...record };
  const requireSlugs = new Set(guidanceSlugs(merged.require));
  for (const slug of guidanceSlugs(merged.avoid)) {
    if (requireSlugs.has(slug)) {
      throw new Error(
        `useIntent: ${JSON.stringify(slug)} is declared in both require and avoid — contradictory intent`,
      );
    }
  }
  Object.assign(frame.intent, record);
}

/**
 * Declares one or more resources this trait uses — adds them to the trait's
 * explicit resource declarations and marks them referenced, so a resource
 * consumed only through `useResource` (never interpolated into a prompt) is
 * not flagged by the never-referenced check.
 * @example `useResource(styleGuide)`
 */
export function useResource(value: ResourceHandle | readonly ResourceHandle[]): void {
  const frame = currentTraitFrame("useResource");
  const values = Array.isArray(value) ? value : [value];
  values.forEach((item, index) => {
    if (item === undefined) {
      throw new Error(`useResource: array entry [${index}] is undefined — check for a typo'd reference`);
    }
    if (metaOf(item)?.kind !== "resource") {
      throw new Error(`useResource: array entry [${index}] is not a resource handle`);
    }
    frame.resources.push(item);
  });
}

function camelToKebabCase(value: string): string {
  return value.replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase();
}

/** `ctx.input.*`: derives the declared input port's id from the accessed property name and returns an interpolatable ref, recording the access for finalize-time validation. */
function inputProxy(frame: TraitFrame): Record<string, unknown> {
  return new Proxy(
    {},
    {
      get(_target, prop): unknown {
        if (typeof prop === "symbol") return undefined;
        const id = camelToKebabCase(prop);
        frame.inputAccessed.add(id);
        return ref.port(id);
      },
    },
  );
}

/** The `ctx` parameter passed to a trait function. */
export interface TraitFunctionContext {
  /** Proxy over this trait's declared input ports — `ctx.input.diff` resolves to the declared `port:diff` input port. */
  readonly input: Record<string, unknown>;
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return typeof value === "object" && value !== null && typeof (value as { then?: unknown; }).then === "function";
}

/** Rebuilds a full declared port handle from its recorded mint, so a `ctx.input` access can be routed through `TraitFields.port` without needing the original module-scope handle reference. */
function reconstructDeclaredPortHandle(mint: FrameMint): PortHandle {
  return withMeta<Record<string, never>, "port">({}, {
    kind: "port",
    ref: mint.ref,
    declaration: mint.declaration as MetaDeclaration,
  });
}

function outputPortFromReturn(key: string, value: unknown): PortHandle {
  const meta = metaOf(value);
  if (meta?.kind !== "slot" || meta.declaration === undefined) {
    throw new Error(
      `evaluateTraitFunction: return value ${JSON.stringify(key)} must be a slot handle written by a step, got ${
        value === null ? "null" : typeof value
      }`,
    );
  }
  const schemaRef = (meta.declaration as { readonly schema?: unknown; }).schema;
  if (typeof schemaRef !== "string") {
    throw new Error(`evaluateTraitFunction: return value ${JSON.stringify(key)}'s slot declares no schema`);
  }
  return port.output.of({ id: camelToKebabCase(key), schema: schemaRef, value: value as SlotHandle });
}

/**
 * After assembly, diffs every resource/signal/port/slot minted while this
 * trait frame was active against the ids the assembled trait actually
 * merged in — any minted declaration absent from the merged sets is a build
 * error naming kind, id, and (when known) the authoring file.
 */
function checkNeverReferenced(
  mints: readonly FrameMint[],
  merged: Partial<Record<string, readonly { readonly id?: unknown; }[]>>,
): void {
  const referencedIds = new Map<string, Set<string>>();
  for (const kind of ["resource", "signal", "port", "slot"] as const) {
    referencedIds.set(
      kind,
      new Set((merged[kind] ?? []).flatMap((item) => (typeof item.id === "string" ? [item.id] : []))),
    );
  }
  const problems: string[] = [];
  for (const mint of mints) {
    if (!referencedIds.get(mint.kind)?.has(mint.id)) {
      const location = mint.file === undefined ? "" : ` (${mint.file})`;
      problems.push(`${mint.kind} ${JSON.stringify(mint.id)}${location}`);
    }
  }
  if (problems.length > 0) {
    throw new Error(`defineTrait: declared but never referenced — ${problems.join(", ")}`);
  }
}

/**
 * Evaluates a full-file functional trait entry: opens the build stack,
 * constructs `ctx`, runs `fn` synchronously, and assembles the result into a
 * `TraitFields` value lowered through the existing `trait()` path. Returns
 * the same `{ draft, __map, diagnostics, authoredDeclarations }` envelope
 * shape `toDraftJsonWithSourceMap` produces for an object-layer trait, so
 * everything downstream (drift, digest, lock, source maps) is unchanged.
 */
export function evaluateTraitFunction(
  fn: (ctx: TraitFunctionContext) => unknown,
): ReturnType<typeof toDraftJsonWithSourceMap> {
  openBuild();
  openTraitFrame();
  const frame = currentTraitFrame("evaluateTraitFunction");
  const ctx: TraitFunctionContext = { input: inputProxy(frame) };
  let result: unknown;
  let items: readonly RegisteredItem[];
  let traitFrame: TraitFrame;
  try {
    result = fn(ctx);
  } finally {
    items = closeBuild();
    traitFrame = closeTraitFrame();
  }
  if (isThenable(result)) {
    throw new Error(
      "evaluateTraitFunction: async trait functions are not supported — the function must run synchronously",
    );
  }
  if (!traitFrame.defineTraitCalled) {
    throw new Error(
      "evaluateTraitFunction: defineTrait(...) was never called — every trait function must call it exactly once",
    );
  }

  const declaredInputPorts = new Map(
    traitFrame.mints
      .filter((mint) =>
        mint.kind === "port" && (mint.declaration as { readonly direction?: unknown; }).direction === "input"
      )
      .map((mint) => [mint.id, mint] as const),
  );
  const unknownInputs = [...traitFrame.inputAccessed].filter((id) => !declaredInputPorts.has(id));
  if (unknownInputs.length > 0) {
    const declaredIds = [...declaredInputPorts.keys()].sort();
    throw new Error(
      `ctx.input: unknown input port(s) ${unknownInputs.sort().join(", ")} — declared input ports are: ${
        declaredIds.length === 0 ? "(none)" : declaredIds.join(", ")
      }`,
    );
  }
  const inputPorts = [...traitFrame.inputAccessed].map((id) =>
    // oxlint-disable-next-line typescript/no-non-null-assertion -- every id in inputAccessed passed the unknownInputs check above, so it is a key of declaredInputPorts
    reconstructDeclaredPortHandle(declaredInputPorts.get(id)!)
  );

  let outputPorts: unknown[] = [];
  if (result !== undefined) {
    if (typeof result !== "object" || result === null || Array.isArray(result)) {
      throw new Error(
        "evaluateTraitFunction: the trait function's return value must be a plain object mapping output names to slot handles, or undefined",
      );
    }
    outputPorts = Object.entries(result as Record<string, unknown>).map(([key, value]) =>
      outputPortFromReturn(key, value)
    );
  }

  let procedureHandle: TraitFields["procedure"];
  if (items.length > 0) {
    const description = traitFrame.fields?.procedure;
    if (typeof description !== "string" || description.trim() === "") {
      throw new Error(
        "defineTrait: a procedural trait (one that registers steps) requires defineTrait({ procedure: \"...\" }) — a text description of the procedure",
      );
    }
    procedureHandle = procedureOf({
      description,
      sequence: items.map((registered) => registered.item as SequenceHandle),
    });
  }

  const behavior = Object.keys(traitFrame.behavior).length > 0 ? (traitFrame.behavior as BehaviorFields) : undefined;
  const intent = Object.keys(traitFrame.intent).length > 0 ? (traitFrame.intent as IntentSpec) : undefined;
  const resource = traitFrame.resources.length > 0 ? (traitFrame.resources as ResourceHandle[]) : undefined;
  const ports = [...inputPorts, ...outputPorts] as PortHandle[];

  const name = traitFrame.fields?.name as string | undefined;
  const version = traitFrame.fields?.version as SemVer | undefined;
  const summary = traitFrame.fields?.summary as string | undefined;
  const metadata = traitFrame.fields?.metadata as TraitMetadata | undefined;
  const fields: TraitFields = {
    ...(name === undefined ? {} : { name }),
    ...(version === undefined ? {} : { version }),
    ...(summary === undefined ? {} : { summary }),
    ...(metadata === undefined ? {} : { metadata }),
    ...(behavior === undefined ? {} : { behavior }),
    ...(intent === undefined ? {} : { intent }),
    ...(resource === undefined ? {} : { resource }),
    ...(ports.length === 0 ? {} : { port: ports }),
    ...(procedureHandle === undefined ? {} : { procedure: procedureHandle }),
  };

  const handle = trait(traitFrame.slug as string, fields);
  const merged = (metaOf(handle)?.declarations ?? {}) as Partial<Record<string, readonly { readonly id?: unknown; }[]>>;
  checkNeverReferenced(traitFrame.mints, merged);
  return toDraftJsonWithSourceMap(handle);
}
