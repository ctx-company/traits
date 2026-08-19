/**
 * The full-file functional trait entry (0107): `defineTrait`, the `use*`
 * outcome gates, `ctx.input`, and `evaluateTraitFunction` — the harness call
 * for a `export default function (ctx) { ... }` trait module. Everything
 * here lowers through the existing object-layer `trait()`/
 * `assembleSingleTraitDraft` path (`../trait.js`) — no second emitter (0102
 * Watch).
 */
import type { CanonicalPort, CanonicalSchemaForm, CanonicalSlot } from "../generated.js";
import type { PortHandle, ResourceHandle, SequenceHandle, SlotHandle } from "../handles.js";
import type { DeclKind, JsonObject, MetaDeclaration } from "../meta.js";
import { metaOf, withMeta } from "../meta.js";
import { isSlug, toDraftJsonWithSourceMap } from "../normalize.js";
import { port } from "../port.js";
import { procedure as procedureOf } from "../procedure.js";
import { ref } from "../ref.js";
import { idFromTitle } from "../sequence.js";
import type { SessionTitleSinkInput } from "../sink.js";
import type { Behavior, Intent, SemVer, TraitFields, TraitMetadata } from "../trait.js";
import { trait } from "../trait.js";
import { variant as variantOf } from "../variant.js";
import type { FrameMint, RegisteredItem, TraitFrame } from "./context.js";
import { closeBuild, closeTraitFrame, currentTraitFrame, openBuild, openTraitFrame } from "./context.js";
import { isThenable } from "./internal.js";

/** `defineTrait`'s own fields: identity and metadata only — everything else is derived from `use*` calls, `ctx.input`, steps, and the return statement. */
export interface DefineTraitFields {
  readonly name?: string;
  readonly version?: SemVer;
  /** The required agent+user prose. Also supplies the procedure's internal description when the trait registers steps. */
  readonly description?: string;
  /** Optional user-only override, defaulting to `description` when absent. */
  readonly summary?: string;
  readonly metadata?: TraitMetadata;
}

const DEFINE_TRAIT_KEYS: readonly string[] = ["name", "version", "description", "summary", "metadata"];
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
  if ("procedure" in fields && !allowed.includes("procedure")) {
    throw new Error(
      `${caller}: "procedure" is no longer an authoring field — use "description" instead; its text also supplies the procedure's internal description when the trait registers steps`,
    );
  }
  const unknown = Object.keys(fields).filter((key) => !allowed.includes(key));
  if (unknown.length > 0) {
    throw new Error(`${caller}: unknown field(s) ${unknown.join(", ")} — expected one of ${allowed.join(", ")}`);
  }
}

/** Catches the enum-typo case (e.g. `intent.RubberStampReviw`, which evaluates to `undefined`) as an array entry instead of a silently dropped guidance item. */
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
export function defineTrait(name: string, fields: DefineTraitFields = {}): void {
  const frame = currentTraitFrame("defineTrait");
  if (frame.frameKind === "variant") {
    throw new Error(
      "defineTrait: called inside a variant function — a variant module declares itself with defineVariant; identity and version belong to the family",
    );
  }
  if (frame.defineTraitCalled) {
    throw new Error("defineTrait: called more than once — a trait function declares its identity exactly once");
  }
  // Same practice as step titles (0109 F2 direction): the first argument is
  // the NAME; the canonical id is its kebab-casing. A bare slug derives to
  // itself, so slug-first call sites keep byte-identical canonicals.
  const slug = idFromTitle(name);
  assertKnownKeys(fields as Record<string, unknown>, DEFINE_TRAIT_KEYS, "defineTrait");
  assertJsonLiteral(fields, "defineTrait fields");
  frame.defineTraitCalled = true;
  frame.slug = slug;
  // The first argument becomes the `name` field unless one was given
  // explicitly. It is injected even when it is already a bare slug: `name`
  // is REQUIRED on the canonical, so skipping the injection produced a draft
  // that could not decode -- `ctx traits create work` (a lowercase name
  // kebab-cases to itself) failed with "invalid manifest at root: missing
  // field `name`" while `create Work` succeeded. A family shell is unharmed
  // either way, because each variant declares its own name.
  frame.fields = fields.name === undefined ? { ...fields, name } : { ...fields };
}

/** `defineVariant`'s own fields: metadata and descriptions only — no `version` (the family owns it); everything else is derived from `use*` calls, steps, and the return statement, exactly like `defineTrait`. */
export interface DefineVariantFields {
  /** Explicit canonical variant id (kebab slug). When present, the first argument is purely the display name — `defineVariant("Implement (Quick)", { id: "quick" })` — so a composed display form never mints a composed public id. */
  readonly id?: string;
  readonly name?: string;
  /** The required agent+user prose. */
  readonly description?: string;
  /** Optional internal procedure description, defaulting to `description`. */
  readonly procedureDescription?: string;
  /** Optional user-only override, defaulting to `description` when absent. */
  readonly summary?: string;
  readonly metadata?: TraitMetadata;
}

const DEFINE_VARIANT_KEYS: readonly string[] = [
  "id",
  "name",
  "description",
  "procedureDescription",
  "summary",
  "metadata",
];

/**
 * Declares a hook-style variant module's identity — exactly once, inside a
 * variant function evaluated via its family's `useVariant` binding. The slug
 * is the family map key, so it lives with the variant it names instead of
 * drifting in the family shell.
 * @example `defineVariant("quick", { name: "Refactor (Quick)", summary: "Fast pass.", procedure: "..." })`
 */
export function defineVariant(name: string, fields: DefineVariantFields = {}): void {
  const frame = currentTraitFrame("defineVariant");
  if (frame.frameKind === "trait") {
    throw new Error(
      "defineVariant: called inside a trait function — a family shell declares itself with defineTrait and binds variants with useVariant",
    );
  }
  if (frame.defineTraitCalled) {
    throw new Error("defineVariant: called more than once — a variant function declares its identity exactly once");
  }
  // Name -> kebab-cased canonical id, exactly like defineTrait and step
  // titles; a bare slug derives to itself. NOTE: the variant id is a public
  // surface (`ctx traits run family:variant`, manifest keys, generated
  // paths), so a variant is named by its own word ("Strict"), never the
  // composed display form ("Refactor (Strict)" would mint refactor-strict).
  assertKnownKeys(fields as Record<string, unknown>, DEFINE_VARIANT_KEYS, "defineVariant");
  assertJsonLiteral(fields, "defineVariant fields");
  const { id, ...rest } = fields;
  const slug = id ?? idFromTitle(name);
  if (id !== undefined && id !== idFromTitle(id)) {
    throw new Error(`defineVariant: id ${JSON.stringify(id)} is not a kebab slug`);
  }
  frame.defineTraitCalled = true;
  frame.slug = slug;
  frame.fields = rest.name === undefined && name !== slug ? { ...rest, name } : { ...rest };
}

/** The chain handle `useVariant` returns — `.default()` marks that binding the family default, exactly once per family. */
export interface UseVariantHandle {
  default(): void;
}

/**
 * Binds one variant into the family being defined. A hook-style variant is a
 * function module (its `defineVariant` call names the family key); a legacy
 * object-style `variant({...})` handle is bridged in with an explicit `key`
 * so a family can migrate one module at a time. Evaluation is deferred:
 * bindings are recorded here and each variant function runs in its own frame
 * after the family function returns — frames never nest.
 * @example `useVariant(quick)` · `useVariant(defaultVariant).default()` · `useVariant(legacyHandle, "strict")`
 */
export function useVariant(value: unknown, key?: string): UseVariantHandle {
  const frame = currentTraitFrame("useVariant");
  if (frame.frameKind === "variant") {
    throw new Error("useVariant: called inside a variant function — only a family shell binds variants");
  }
  if (typeof value === "function") {
    if (key !== undefined) {
      throw new Error("useVariant: a variant function carries its own key via defineVariant — do not pass one here");
    }
  } else if (metaOf(value) !== undefined || (value !== null && typeof value === "object")) {
    if (key === undefined || !isSlug(key)) {
      throw new Error(
        'useVariant: an object-style variant handle needs an explicit family key — useVariant(handle, "quick")',
      );
    }
  } else {
    throw new Error(
      `useVariant: expected a variant function or a variant({...}) handle, got ${
        value === null ? "null" : typeof value
      }`,
    );
  }
  const binding = { value, key, isDefault: false };
  frame.boundVariants.push(binding);
  return {
    default(): void {
      binding.isDefault = true;
    },
  };
}

/** A built-in guidance reference (e.g. `intent.CiteEvidence`) — either a bare kebab-case slug or `{ id }`. */
interface GuidanceRef {
  readonly id?: string;
}

function guidanceSlug(value: unknown): string | undefined {
  if (typeof value === "string") return value;
  if (value !== null && typeof value === "object" && typeof (value as GuidanceRef).id === "string") {
    return (value as GuidanceRef).id;
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
export function useBehavior(fields: Behavior): void {
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
 * @example `useIntent({ require: [intent.CiteEvidence] })`
 */
export function useIntent(fields: Intent): void {
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

/** Rebuilds a full declared port handle from its recorded mint, so a `ctx.input` access can be routed through `TraitFields.port` without needing the original module-scope handle reference. */
function reconstructDeclaredPortHandle(mint: FrameMint): PortHandle {
  return withMeta<Record<string, never>, "port">(
    {},
    {
      kind: "port",
      ref: mint.ref,
      declaration: mint.declaration as MetaDeclaration,
    },
  );
}

function outputPortFromReturn(key: string, value: unknown): PortHandle {
  const meta = metaOf(value);
  // A fully-declared output port passes through untouched, so a decorated
  // port (title, description, optional, format) loses nothing by being
  // returned instead of listed — the fidelity a slot-derived port cannot
  // carry.
  if (meta?.kind === "port") {
    const direction = (meta.declaration as CanonicalPort | undefined)?.direction;
    if (direction !== "output") {
      throw new Error(
        `evaluateTraitFunction: return value ${JSON.stringify(
          key,
        )} is an input port — only output ports and slots may be returned`,
      );
    }
    return value as PortHandle;
  }
  if (meta?.kind !== "slot" || meta.declaration === undefined) {
    throw new Error(
      `evaluateTraitFunction: return value ${JSON.stringify(key)} must be a slot handle written by a step, got ${
        value === null ? "null" : typeof value
      }`,
    );
  }
  const schemaRef: CanonicalSchemaForm | undefined = (meta.declaration as CanonicalSlot).schema;
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
  merged: Partial<Record<DeclKind, readonly JsonObject[]>>,
): void {
  const referencedIds = new Map<string, Set<string>>();
  for (const kind of ["resource", "signal", "port", "slot", "setting"] as const) {
    referencedIds.set(
      kind,
      new Set((merged[kind] ?? []).flatMap((item) => (typeof item["id"] === "string" ? [item["id"]] : []))),
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
/** One evaluated function frame: the closed frame plus everything its run produced. */
interface EvaluatedFrame {
  readonly traitFrame: TraitFrame;
  readonly items: readonly RegisteredItem[];
  readonly sessionTitleSink: unknown;
  readonly result: unknown;
}

/** Opens a build+frame pair, runs `fn` synchronously, and closes both — the one evaluation shape trait and variant functions share. */
function runFrame(fn: (ctx: TraitFunctionContext) => unknown, frameKind: "trait" | "variant"): EvaluatedFrame {
  openBuild();
  openTraitFrame(frameKind);
  const frame = currentTraitFrame("evaluateTraitFunction");
  const ctx: TraitFunctionContext = { input: inputProxy(frame) };
  let result: unknown;
  let items: readonly RegisteredItem[];
  let sessionTitleSink: unknown;
  let traitFrame: TraitFrame;
  try {
    result = fn(ctx);
  } finally {
    ({ items, sessionTitleSink } = closeBuild());
    traitFrame = closeTraitFrame();
  }
  if (isThenable(result)) {
    throw new Error(
      "evaluateTraitFunction: async trait functions are not supported — the function must run synchronously",
    );
  }
  return { traitFrame, items, sessionTitleSink, result };
}

export function evaluateTraitFunction(
  // `unknown`, not the `Record<string, SlotHandle> | undefined` return contract: an invalid
  // return value must fail with `outputPortFromReturn`'s runtime message (naming the bad key),
  // not a compiler error — the same validated-boundary idiom as a type guard's `unknown` param.
  fn: (ctx: TraitFunctionContext) => unknown,
): ReturnType<typeof toDraftJsonWithSourceMap> | ReturnType<typeof trait> {
  const { traitFrame, items, sessionTitleSink, result } = runFrame(fn, "trait");
  if (!traitFrame.defineTraitCalled) {
    throw new Error(
      "evaluateTraitFunction: defineTrait(...) was never called — every trait function must call it exactly once",
    );
  }
  if (traitFrame.boundVariants.length > 0) {
    return assembleFamily(traitFrame, items);
  }
  return assembleSingleTrait(traitFrame, items, sessionTitleSink, result);
}

function assembleSingleTrait(
  traitFrame: TraitFrame,
  items: readonly RegisteredItem[],
  sessionTitleSink: unknown,
  result: unknown,
): ReturnType<typeof toDraftJsonWithSourceMap> {
  const declaredInputPorts = new Map(
    traitFrame.mints
      .filter((mint) => mint.kind === "port" && (mint.declaration as CanonicalPort).direction === "input")
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
    reconstructDeclaredPortHandle(declaredInputPorts.get(id)!),
  );

  let outputPorts: PortHandle[] = [];
  if (result !== undefined) {
    if (typeof result !== "object" || result === null || Array.isArray(result)) {
      throw new Error(
        "evaluateTraitFunction: the trait function's return value must be a plain object mapping output names to slot handles, or undefined",
      );
    }
    outputPorts = Object.entries(result as Record<string, unknown>).map(([key, value]) =>
      outputPortFromReturn(key, value),
    );
  }

  let procedureHandle: TraitFields["procedure"];
  if (items.length > 0) {
    const description = traitFrame.fields?.description;
    if (typeof description !== "string" || description.trim() === "") {
      throw new Error(
        'defineTrait: a procedural trait (one that registers steps) requires defineTrait({ description: "..." }) — a text description of the trait',
      );
    }
    procedureHandle = procedureOf({
      description,
      sequence: items.map((registered) => registered.item as SequenceHandle),
    });
  }

  const behavior = Object.keys(traitFrame.behavior).length > 0 ? (traitFrame.behavior as Behavior) : undefined;
  const intent = Object.keys(traitFrame.intent).length > 0 ? (traitFrame.intent as Intent) : undefined;
  const resource = traitFrame.resources.length > 0 ? (traitFrame.resources as ResourceHandle[]) : undefined;
  const ports = [...inputPorts, ...outputPorts] as PortHandle[];

  const name = traitFrame.fields?.name as string | undefined;
  const version = traitFrame.fields?.version as SemVer | undefined;
  const description = traitFrame.fields?.description as string | undefined;
  const summary = traitFrame.fields?.summary as string | undefined;
  const metadata = traitFrame.fields?.metadata as TraitMetadata | undefined;
  const fields: TraitFields = {
    ...(name === undefined ? {} : { name }),
    ...(version === undefined ? {} : { version }),
    ...(description === undefined ? {} : { description }),
    ...(summary === undefined ? {} : { summary }),
    ...(metadata === undefined ? {} : { metadata }),
    ...(behavior === undefined ? {} : { behavior }),
    ...(intent === undefined ? {} : { intent }),
    ...(resource === undefined ? {} : { resource }),
    ...(ports.length === 0 ? {} : { port: ports }),
    ...(procedureHandle === undefined ? {} : { procedure: procedureHandle }),
    ...(sessionTitleSink === undefined ? {} : { sink: { sessionTitle: sessionTitleSink as SessionTitleSinkInput } }),
  };

  const handle = trait(traitFrame.slug as string, fields);
  const merged: Partial<Record<DeclKind, readonly JsonObject[]>> = metaOf(handle)?.declarations ?? {};
  checkNeverReferenced(traitFrame.mints, merged);
  return toDraftJsonWithSourceMap(handle);
}

/**
 * Assembles the fields a hook-style VARIANT function produced — the same
 * derivation `assembleSingleTrait` performs, minus identity/version (the
 * family owns both) and minus `checkNeverReferenced` (a variant handle
 * carries no merged declarations until family flattening, which is where
 * unreferenced declarations already fail).
 */
function assembleVariantFields(evaluated: EvaluatedFrame): Record<string, unknown> {
  const { traitFrame, items, sessionTitleSink, result } = evaluated;
  const declaredInputPorts = new Map(
    traitFrame.mints
      .filter((mint) => mint.kind === "port" && (mint.declaration as CanonicalPort).direction === "input")
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
    // oxlint-disable-next-line typescript/no-non-null-assertion -- every id in inputAccessed passed the unknownInputs check above
    reconstructDeclaredPortHandle(declaredInputPorts.get(id)!),
  );

  let outputPorts: PortHandle[] = [];
  if (result !== undefined) {
    if (typeof result !== "object" || result === null || Array.isArray(result)) {
      throw new Error(
        "defineVariant: the variant function's return value must be a plain object mapping output names to slot/output-port handles, or undefined",
      );
    }
    outputPorts = Object.entries(result as Record<string, unknown>).map(([key, value]) =>
      outputPortFromReturn(key, value),
    );
  }

  let procedureHandle: TraitFields["procedure"];
  if (items.length > 0) {
    const description = traitFrame.fields?.procedureDescription ?? traitFrame.fields?.description;
    if (typeof description !== "string" || description.trim() === "") {
      throw new Error(
        'defineVariant: a procedural variant (one that registers steps) requires defineVariant(slug, { description: "..." }) — a text description of the variant',
      );
    }
    procedureHandle = procedureOf({
      description,
      sequence: items.map((registered) => registered.item as SequenceHandle),
    });
  }

  const behavior = Object.keys(traitFrame.behavior).length > 0 ? (traitFrame.behavior as Behavior) : undefined;
  const intent = Object.keys(traitFrame.intent).length > 0 ? (traitFrame.intent as Intent) : undefined;
  const resource = traitFrame.resources.length > 0 ? (traitFrame.resources as ResourceHandle[]) : undefined;
  const ports = [...inputPorts, ...outputPorts] as PortHandle[];
  const name = traitFrame.fields?.name as string | undefined;
  const description = traitFrame.fields?.description as string | undefined;
  const summary = traitFrame.fields?.summary as string | undefined;
  const metadata = traitFrame.fields?.metadata as TraitMetadata | undefined;
  return {
    ...(name === undefined ? {} : { name }),
    ...(description === undefined ? {} : { description }),
    ...(summary === undefined ? {} : { summary }),
    ...(metadata === undefined ? {} : { metadata }),
    ...(behavior === undefined ? {} : { behavior }),
    ...(intent === undefined ? {} : { intent }),
    ...(resource === undefined ? {} : { resource }),
    ...(ports.length === 0 ? {} : { port: ports }),
    ...(procedureHandle === undefined ? {} : { procedure: procedureHandle }),
    ...(sessionTitleSink === undefined ? {} : { sink: { sessionTitle: sessionTitleSink as SessionTitleSinkInput } }),
  };
}

/**
 * Finalizes a hook-style FAMILY: evaluates every `useVariant`-bound variant
 * function in its own frame (deferred — frames never nest), bridges
 * object-style handles through untouched, validates key uniqueness and the
 * exactly-one-default rule, and lowers through the object-layer
 * `trait(slug, { variants })` path — no second emitter (0102 Watch). The
 * returned family handle is what the build script's `resolveTraitFamily`
 * branch consumes, exactly as for an object-style family module.
 */
/**
 * Merges a family shell's `useIntent`/`useResource` facets into one bound
 * variant's own frame before `assembleVariantFields` reads it — shell
 * entries first, variant's own entries preserved as the higher-priority
 * addition, deduped per facet by guidance/resource ref so a variant that
 * redeclares a shell facet does not double it. Reuses `useIntent`'s
 * require/avoid contradiction check across the merged result, since a
 * shell `require` and a variant `avoid` (or vice versa) is exactly as
 * contradictory as two same-frame calls.
 */
function mergeShellFacetsIntoVariant(familyFrame: TraitFrame, variantFrame: TraitFrame): void {
  if (familyFrame.resources.length > 0) {
    const existingRefs = new Set(variantFrame.resources.map((resource) => metaOf(resource)?.ref));
    const additions = familyFrame.resources.filter((resource) => !existingRefs.has(metaOf(resource)?.ref));
    variantFrame.resources.unshift(...additions);
  }
  const shellIntent = familyFrame.intent as Record<string, unknown>;
  const variantIntent = variantFrame.intent as Record<string, unknown>;
  for (const key of Object.keys(shellIntent)) {
    const shellValue = shellIntent[key];
    const variantValue = variantIntent[key];
    if (variantValue === undefined) {
      variantIntent[key] = shellValue;
      continue;
    }
    const shellList = Array.isArray(shellValue) ? shellValue : [shellValue];
    const variantList = Array.isArray(variantValue) ? variantValue : [variantValue];
    const seen = new Set(guidanceSlugs(variantList));
    const additions = shellList.filter((item) => {
      const slug = guidanceSlug(item);
      return slug === undefined || !seen.has(slug);
    });
    variantIntent[key] = [...additions, ...variantList];
  }
  const requireSlugs = new Set(guidanceSlugs(variantIntent.require));
  for (const slug of guidanceSlugs(variantIntent.avoid)) {
    if (requireSlugs.has(slug)) {
      throw new Error(
        `useIntent: ${JSON.stringify(
          slug,
        )} is declared in both require and avoid once the family shell's useIntent is merged into variant ${JSON.stringify(
          variantFrame.slug,
        )} — contradictory intent`,
      );
    }
  }
}

function assembleFamily(familyFrame: TraitFrame, familyItems: readonly RegisteredItem[]): ReturnType<typeof trait> {
  if (familyItems.length > 0) {
    throw new Error("useVariant: a family shell must not register its own steps — procedures belong to the variants");
  }
  const shellHasDeclaredFacets = Object.keys(familyFrame.intent).length > 0 || familyFrame.resources.length > 0;
  const variants: Record<string, unknown> = {};
  let defaultKey: string | undefined;
  for (const binding of familyFrame.boundVariants) {
    let key: string;
    let handle: unknown;
    if (typeof binding.value === "function") {
      const evaluated = runFrame(binding.value as (ctx: TraitFunctionContext) => unknown, "variant");
      if (!evaluated.traitFrame.defineTraitCalled) {
        throw new Error(
          "useVariant: a bound variant function never called defineVariant(...) — every variant module declares its identity exactly once",
        );
      }
      mergeShellFacetsIntoVariant(familyFrame, evaluated.traitFrame);
      key = evaluated.traitFrame.slug as string;
      handle = variantOf(assembleVariantFields(evaluated) as Parameters<typeof variantOf>[0]);
    } else {
      if (shellHasDeclaredFacets) {
        throw new Error(
          "useVariant: the family shell declares useIntent/useResource, but this variant is bound as an object-style handle — object-style handles bypass frame evaluation, so the shell's facets cannot reach it. Bind this variant as a function instead, or move the shell's useIntent/useResource calls into each variant.",
        );
      }
      key = binding.key as string;
      handle = binding.value;
    }
    if (key in variants) {
      throw new Error(`useVariant: duplicate variant key ${JSON.stringify(key)} — family keys must be unique`);
    }
    if (binding.isDefault) {
      if (defaultKey !== undefined) {
        throw new Error(
          `useVariant: both ${JSON.stringify(defaultKey)} and ${JSON.stringify(
            key,
          )} marked default — exactly one variant may be the family default`,
        );
      }
      defaultKey = key;
      handle = (handle as { default(): unknown }).default();
    }
    variants[key] = handle;
  }
  if (defaultKey === undefined) {
    throw new Error("useVariant: no variant marked default — chain .default() onto exactly one useVariant(...) call");
  }
  const rawName = familyFrame.fields?.name as string | undefined;
  // A family shell's display name lives in trait.toml and trait() rejects
  // `name` on families. The name defineTrait derived the family id from
  // (it kebab-cases back to the slug) has served its purpose — drop it; a
  // genuinely different explicit name still reaches trait() and errors.
  const name = rawName !== undefined && idFromTitle(rawName) === familyFrame.slug ? undefined : rawName;
  const version = familyFrame.fields?.version as SemVer | undefined;
  const description = familyFrame.fields?.description as string | undefined;
  const summary = familyFrame.fields?.summary as string | undefined;
  const metadata = familyFrame.fields?.metadata as TraitMetadata | undefined;
  return trait(
    familyFrame.slug as string,
    {
      ...(name === undefined ? {} : { name }),
      ...(version === undefined ? {} : { version }),
      ...(description === undefined ? {} : { description }),
      ...(summary === undefined ? {} : { summary }),
      ...(metadata === undefined ? {} : { metadata }),
      variants: variants as TraitFields["variants"],
    } as TraitFields,
  );
}
