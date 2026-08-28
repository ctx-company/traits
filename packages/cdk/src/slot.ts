import { dispatchSlotForEach, recordTraitMint } from "./functional/context.js";
import type { ForEachParam } from "./functional/registrars.js";
import type { JsonObject, JsonValue, WriteOperation } from "./generated.js";
import type {
  DeclaredSlotHandle,
  DeclaredSlotWithFields,
  OptionalSlotRead,
  OutputSinkHandle,
  SequenceHandle,
  SlotHandle,
  SlotSink,
} from "./handles.js";
import { fieldRefProxy as sharedFieldRefProxy, objectSchemaFields } from "./field-refs.js";
import { optionalSlot as optionalSlotInput } from "./input.js";
import { withDeclaration, withHiddenField, withMeta } from "./meta.js";
import { collectMany, compact, slugFromName, validateSlug } from "./normalize.js";
import { ref, refText } from "./ref.js";
import { schemaList } from "./schema.js";
import type { SchemaValue } from "./schema.js";

export interface SlotFields {
  readonly id: string;
  readonly schema: SchemaValue;
  readonly description?: string;
  readonly hint?: string;
  /** Display name, when it must differ from the name's kebab-casing. The same
   * field a port carries. */
  readonly title?: string;
  /**
   * Whether reading this slot blocks. A slot is written by a step, so a read
   * before any step could have written is normally a build error; marking the
   * slot optional says the opposite once, at the declaration, instead of
   * `slot.optional()` at every site that reads it.
   */
  readonly optional?: boolean;
  /**
   * A value the slot holds before any step writes one — so a prompt can
   * interpolate it on the first pass, and a counter can start somewhere.
   * Validated against the slot's own schema when the trait is built.
   */
  readonly default?: JsonValue;
}

/**
 * Slot builder call signatures: declares a typed, procedure-LOCAL place
 * where a sequence step's structured or free-form output lands, so a later
 * step (or an output port) can read it.
 *
 * Hand-writing a slot as raw JSON risks a step writing to one id and a later
 * step (or the output port's `value`) reading a differently-spelled one —
 * caught only when the run fails to find the value. `slot(...)` returns a
 * typed handle: pass the same handle as a step's `output` and the next
 * step's interpolated input, and a mismatch fails at `tsc`.
 *
 * **Slots vs ports**: a slot is internal working state — it exists only for
 * the lifetime of the procedure and is never itself a cross-trait boundary
 * value. A `port` (see `port.ts`) IS that boundary: an input the trait's
 * caller supplies, or an output the caller receives. The usual pattern is a
 * slot that a step writes to, then an output `port`'s `value` field points
 * at that slot to expose it — the port is what's visible outside the trait,
 * the slot is what's visible only between steps inside it.
 *
 * @param schemaValue The represented value schema.
 * @example
 * ```ts
 * const diff = port.input.text({ id: "diff" });
 * const review = slot({
 *   id: "review",
 *   schema: schema.object("code-review-scaffold", { verdict: schema.verdict() }),
 * });
 * sequence.prompt("review", { text: input.prompt`Review ${diff}.`, output: review });
 * ```
 * @see {@link SlotFields}
 * @see {@link port}
 */
export interface SlotFunction {
  /**
   * Full-form slot declaration with an explicit schema. When `schema` is an
   * object schema (`schema.object`/`extend`/`template`), the returned handle
   * also exposes one typed `FieldRef` per declared field —
   * `slot.foo`/`slot["exit-code"]` — for `condition.equals`/`fieldEquals`.
   * @example `slot({ id: "review", schema: schema.text() })`
   */
  <Value>(fields: SlotFields & { readonly schema: SchemaValue<Value> }): DeclaredSlotWithFields<Value>;
  /** Text-schema slot shorthand; a bare string is shorthand for `{ id: value }`. @example `slot.text("summary")` */
  text(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<string>;
  /** Boolean-schema slot shorthand. @example `slot.boolean("approved")` */
  boolean(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<boolean>;
  /** Numeric-schema slot shorthand. @example `slot.number("retry-count")` */
  number(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<number>;
  /** Any-JSON-schema slot shorthand, for output shapes not worth declaring precisely. @example `slot.any("raw")` */
  any(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<JsonValue>;
  /**
   * A slot with an explicit schema, named first. The barebones form —
   * every shorthand above is this with the schema filled in.
   * @example `slot.of("Review Verdict", reviewVerdictSchema)`
   * @example `slot.of("Retry Count", schema.number(), { default: 0 })`
   */
  of<Value>(
    name: string,
    schemaValue: SchemaValue<Value>,
    fields?: Omit<SlotFields, "schema" | "id">,
  ): DeclaredSlotWithFields<Value>;
  /** Curried list-slot factory: bind the element schema once, declare several slots of that list shape. @example `const findingsList = slot.list(schema.text()); const findings = findingsList("findings");` */
  list<Value>(
    schemaValue: SchemaValue<Value>,
  ): (value: string | Omit<SlotFields, "schema">) => DeclaredSlotHandle<Value[]>;
  /** List-slot shorthand with the element schema and id/fields both given. @example `slot.list(schema.text(), "findings")` */
  list<Value>(schemaValue: SchemaValue<Value>, value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<Value[]>;
  /** List-of-text-slot shorthand, the common case of `slot.list(schema.text(), ...)`. @example `slot.texts("notes")` */
  texts(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<string[]>;
  /** List-of-number-slot shorthand, the numeric sibling of {@link SlotFunction.texts}. @example `slot.numbers("durations")` */
  numbers(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<number[]>;
}

/**
 * Brand key marking an `operation.literal(...)` result so `project`
 * normalization can distinguish it from a plain object destined for
 * `refText`/schema declaration collection.
 */
const LITERAL_PROJECTION_SOURCE_BRAND: unique symbol = Symbol("literalProjectionSource");

/**
 * A canonical typed literal `project` projection source (P431):
 * `{ source: operation.literal(1), destination: ... }`. Executes entirely
 * inside the core runtime — it reads no slot, contributes no sequence input,
 * and is validated against the destination's write schema at build time.
 * @see {@link OperationFunction.literal}
 */
export interface LiteralProjectionSource<Value extends JsonValue = JsonValue> {
  readonly [LITERAL_PROJECTION_SOURCE_BRAND]: true;
  readonly literal: Value;
}

/** Narrows to an `operation.literal(...)` result. */
export function isLiteralProjectionSource(value: unknown): value is LiteralProjectionSource {
  return typeof value === "object" && value !== null && LITERAL_PROJECTION_SOURCE_BRAND in value;
}

export interface OperationFunction {
  /**
   * The `.with` write mode that appends to a list slot: accumulation is a
   * WRITE MODE, never a read-format-rewrite. Two steps each writing
   * `findings.with(operation.Append)` yield both entries, in order — never
   * a step reading the slot's current value, string-concatenating its own
   * addition, and writing the whole thing back (a race under replay, and a
   * format every producer must agree on by convention). A `schema:text`
   * slot cannot take `Append` (core requires an array slot schema) — accumulate
   * text as `slot.list(schema.text(), ...)`, one formatted entry per append.
   * @example `findings.with(operation.Append)`
   */
  readonly Append: "append";
  /** The `.with` write mode that merges an object into an object slot. */
  readonly Merge: "merge";
  /** The `.with` write mode that replaces the slot's whole value (the default). */
  readonly Replace: "replace";
  /** The `.with` write mode that increments a numeric slot. */
  readonly Increment: "increment";
  /**
   * Builds the `set-field` write-mode value for `.with`: writes one
   * field of an object slot without touching the rest of it.
   * @example `review.with(operation.SetField("status"))`
   */
  SetField(field: string): Extract<WriteOperation, { readonly "set-field": string }>;
  /** Appends a step's output to a list slot instead of replacing it. @example `findings.with(operation.Append)` */
  over<Value>(slotHandle: SlotHandle<Value[]>, operation: "append"): OutputSinkHandle<Value>;
  /** Increments a numeric slot by a step's numeric output. @example `retryCount.with(operation.Increment)` */
  over(slotHandle: SlotHandle<number>, operation: "increment"): OutputSinkHandle<number>;
  /** Merges a step's object output into an object slot, or writes one field via `operation.SetField`. @example `review.with(operation.Merge)` */
  over<Value>(
    slotHandle: SlotHandle<Value>,
    operation: "merge" | Extract<WriteOperation, { readonly "set-field": string }>,
  ): OutputSinkHandle<JsonValue>;
  /** Replaces the slot's whole value (the default write mode — same as passing the slot directly as `output`). @example `review.with(operation.Replace)` */
  over<Value>(slotHandle: SlotHandle<Value>, operation?: "replace"): OutputSinkHandle<Value>;
  /**
   * Declares a canonical typed literal as a `project` step's projection
   * source, instead of reading an existing slot (P431).
   * `{ source: operation.literal(1), destination: retryCount }` initializes
   * `retryCount` to `1` deterministically, entirely inside the core runtime —
   * no agent frame, command activation, or process spawn. The literal is
   * validated against the destination's effective write schema when the
   * trait is built; it must not be combined with `field`.
   * @example `sequence.project("init", { projections: [{ source: operation.literal(0), destination: retryCount }] })`
   */
  literal<Value extends JsonValue>(value: Value): LiteralProjectionSource<Value>;
}

/**
 * Declares a typed procedure-local slot — not a cross-trait boundary value;
 * see {@link port} for that.
 * @param fields Slot identity and schema.
 * @example `slot.text("result")`
 * @see {@link DeclaredSlotHandle.with}
 * @see {@link port}
 */
function slotFn<Value>(fields: SlotFields & { readonly schema: SchemaValue<Value> }): DeclaredSlotWithFields<Value> {
  // `slotOf` stays non-generic and its return may be an object-schema field
  // proxy (`objectSlotFieldProxy`) rather than a bare handle — neither is
  // statically distinguishable from `DeclaredSlotWithFields<Value>` without running
  // it, so this generic wrapper mints the phantom once.
  return slotOf(fields) as DeclaredSlotWithFields<Value>;
}

// `Object.assign`'s typing doesn't preserve a merged value's generic call
// signature precisely (see `port.ts`'s identical note) — `slotFn` is
// independently checked as a real generic above; this is the one cast
// bridging the merge.
export const slot: SlotFunction = Object.assign(slotFn, {
  text: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, "schema:text"),
  boolean: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, "schema:boolean"),
  number: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, "schema:number"),
  any: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, "schema:any"),
  list: (schemaValue: SchemaValue, value?: string | Omit<SlotFields, "schema">) =>
    value === undefined
      ? (slotValue: string | Omit<SlotFields, "schema">) => slotWithSchema(slotValue, schemaList(schemaValue))
      : slotWithSchema(value, schemaList(schemaValue)),
  of: <Value>(name: string, schemaValue: SchemaValue<Value>, fields?: Omit<SlotFields, "schema" | "id">) =>
    slotOf({ ...fields, id: name, schema: schemaValue }) as DeclaredSlotWithFields<Value>,
  texts: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, schemaList("schema:text")),
  numbers: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, schemaList("schema:number")),
}) as SlotFunction;

/**
 * Selects how a sequence step's output writes to a slot: replace it
 * (the default — plain `output: slot`), append to a list slot, merge into
 * an object slot, set one field, or increment a number slot.
 *
 * Without this, every step that accumulates into the same list or counter
 * slot would need a hand-written `{ slot, operation }` sink object, with no
 * check that `operation` is a value the runtime actually recognizes.
 * `operation.over(slot, operation.Append)` returns a typed `OutputSinkHandle`
 * usable directly as a sequence step's `output`. `slot.with(operation.Append)`
 * (0210) is the authoring form; this is the object layer it lowers through —
 * the functional layer may not reach around it (0106).
 *
 * @param slotHandle The destination slot.
 * @example
 * ```ts
 * const findings = slot.list(schema.text(), "findings");
 * const retryCount = slot.number("retry-count");
 * const review = slot.text("review");
 * operation.over(findings, operation.Append);
 * ```
 * @see {@link slot}
 */
function operationOver<Value>(slotHandle: SlotHandle<Value[]>, operation: "append"): OutputSinkHandle<Value>;
function operationOver(slotHandle: SlotHandle<number>, operation: "increment"): OutputSinkHandle<number>;
function operationOver<Value>(
  slotHandle: SlotHandle<Value>,
  operation: "merge" | Extract<WriteOperation, { readonly "set-field": string }>,
): OutputSinkHandle<JsonValue>;
function operationOver<Value>(slotHandle: SlotHandle<Value>, operation?: "replace"): OutputSinkHandle<Value>;
function operationOver(slotHandle: SlotHandle, writeOperation: WriteOperation = "replace"): OutputSinkHandle {
  if (writeOperation === "replace") return slotHandle;
  const declaration = compact({ slot: refText(slotHandle, "operation.over.slot"), operation: writeOperation });
  return withMeta(declaration, {
    kind: "output-sink",
    ref: refText(slotHandle, "operation.over.slot"),
    declaration,
    declarations: collectMany([slotHandle]),
  });
}

export const operation: OperationFunction = {
  Append: "append",
  Merge: "merge",
  Replace: "replace",
  Increment: "increment",
  SetField: (field: string) => {
    validateSlug(field, "operation.SetField.field");
    return { "set-field": field } as const;
  },
  over: operationOver,
  literal: function literal<Value extends JsonValue>(value: Value): LiteralProjectionSource<Value> {
    return { [LITERAL_PROJECTION_SOURCE_BRAND]: true, literal: value } as LiteralProjectionSource<Value>;
  },
};

function slotWithSchema(value: string | Omit<SlotFields, "schema">, schemaRef: SchemaValue): DeclaredSlotHandle {
  return slotOf({ ...(typeof value === "string" ? { id: value } : value), schema: schemaRef });
}

/**
 * Attaches the `.optional()`/`.forEach()`/`.with()` augmentation every
 * declared slot handle carries — non-enumerably, so none of it reaches the
 * canonical declaration. Shared by `slotOf` and `mintAutoSlot` (the
 * virtual-slot mint, `sequence.ts`), the third and prior duplicate this
 * factored out (0253.3): a named slot and an auto-named one must not drift.
 */
/**
 * The `.optional()` closure every slot reference carries — a bare
 * `ref.slot(...)` (`lazyForEachItem`'s proxy, before the real item slot
 * exists) or a fully declared handle (`augmentSlotHandle`) alike, so the two
 * paths cannot drift on what "optional" means for a slot.
 */
function slotOptionalClosure(target: SlotHandle): () => OptionalSlotRead {
  return () => optionalSlotInput(target);
}
/**
 * The `.with(...)` closure every slot reference carries — the authoring-form
 * spelling of `operation.over(slot, op)` (0210, 0207 ruling 4), shared the
 * same way as {@link slotOptionalClosure}.
 */
function slotWithClosure(target: SlotHandle): SlotSink<unknown> {
  return ((op?: WriteOperation) => operationOver(target, op as never)) as SlotSink<unknown>;
}

function augmentSlotHandle(resolved: SlotHandle): DeclaredSlotHandle {
  // `.optional()` is the per-SITE optionality wrapper, identical in output to
  // `input.optional(slot)` — optionality has never been a property of the slot
  // itself, so the same slot stays required at one step and optional at
  // another. Attached non-enumerably so it can never serialize into the
  // canonical, and defined here (not on the proxy path alone) so object-schema
  // and scalar slots both carry it.
  const withOptional = withHiddenField(resolved, "optional", slotOptionalClosure(resolved));
  // `.forEach` is the functional layer's `items.forEach` spelling (0106,
  // 0102) — attached the same way as `.optional`, non-enumerable so it never
  // reaches the canonical declaration.
  const withForEach = withHiddenField(
    withOptional,
    "forEach",
    (title: string, body: (item: SlotHandle, loop: ForEachParam) => void) =>
      dispatchSlotForEach(withOptional, title, body) as SequenceHandle,
  );
  // `.with` is the authoring-form spelling of `operation.over(slot, op)`
  // (0210, 0207 ruling 4) — a pure delegation, not a new declaration path,
  // attached the same non-enumerable way so it never reaches the canonical.
  return withHiddenField(withForEach, "with", slotWithClosure(resolved));
}

/**
 * The one slot declaration path: builds the canonical declaration, mints the
 * handle (with its object-schema field-ref proxy, when the schema is one),
 * and augments it with `.optional()`/`.forEach()`/`.with()`. This IS `slotOf`
 * (an author's hand-declared slot) — its body stays inline here, not behind a
 * separate `declareSlot` wrapper, so every pre-existing named-slot mint keeps
 * the exact call-stack depth it had before 0253.3, between the author's mint
 * site and `withDeclaration`'s `captureSourceAnchor()`. `mintAutoSlot` (the
 * virtual-slot lowering behind `output: schema.text()`, 0253.3) calls this
 * directly too, one frame deeper — acceptable, since a virtual slot is a new
 * mint path with no pre-existing depth to preserve. `recordMint: false` is
 * the only behavioral difference: `functional/trait.ts`'s
 * `checkNeverReferenced` diffs author mints against merged declarations, and
 * an auto slot is not an author mint.
 */
function slotOf(fields: SlotFields, options: { readonly recordMint?: boolean } = {}): DeclaredSlotHandle {
  const id = slugFromName(fields.id, "slot.id");
  const declaration = compact({
    id,
    schema: refText(fields.schema, "slot.schema"),
    description: fields.description ?? `Runtime slot ${id}.`,
    hint: fields.hint,
    title: fields.title,
    optional: fields.optional,
    default: fields.default,
  });
  // The handle's own enumerable surface is empty (only field access, via the
  // proxy below for object schemas, is public) — the canonical `{ id,
  // schema, description, hint }` declaration lives only in `meta.declaration`.
  // See `withDeclaration` in `meta.ts` for the shared split every other
  // declaration builder (agent/port/prompt/schema.object/...) uses too.
  const handle = withDeclaration("slot", `slot:${id}`, declaration, {} as JsonObject, {
    declarations: collectMany([fields.schema]),
  });
  if (options.recordMint !== false) recordTraitMint("slot", id, `slot:${id}`, declaration);
  const objectFields = objectSchemaFields(fields.schema);
  const declarations = collectMany([handle]);
  const resolved =
    objectFields === undefined
      ? (handle as SlotHandle)
      : (sharedFieldRefProxy(handle as SlotHandle, "slot", id, [], objectFields, declarations, (nextPath, decls) => ({
          fieldRef: { slotRef: `slot:${id}`, field: nextPath.join(".") },
          declarations: decls,
        })) as SlotHandle);
  return augmentSlotHandle(resolved);
}

/**
 * Mints an auto-named slot handle WITHOUT recording an author mint — see
 * {@link slotOf}.
 * @see {@link SlotFunction}
 */
export function mintAutoSlot<Value>(id: string, schemaValue: SchemaValue<Value>): DeclaredSlotWithFields<Value> {
  return slotOf({ id, schema: schemaValue }, { recordMint: false }) as DeclaredSlotWithFields<Value>;
}

/**
 * A lazily-materialized `items.forEach` item handle (0211): the body
 * receives `proxy` immediately, before the real item slot (whose schema
 * `each.itemSchema(...)` may still declare) is minted. Whole-value uses
 * (interpolation, `.optional()`, `.with(...)`) build against the stable
 * `ref.slot` target — the ref string is already correct even before a real
 * slot exists — but any other field access before `materialize(...)` is a
 * loud build error (`onFieldAccess`), since an inherited/undeclared item
 * schema mints no field refs to read. `registrars.ts`'s `items.forEach`
 * lowering owns the actual mint (`each.itemSchema` or the default fallback
 * at scope close) and calls `materialize` with the real handle exactly once.
 */
export function lazyForEachItem(
  loopId: string,
  onFieldAccess: (prop: string) => never,
): { readonly proxy: SlotHandle; readonly materialize: (real: SlotHandle) => void } {
  const itemRef = ref.slot(`${loopId}-item`);
  let real: SlotHandle | undefined;
  const proxy = new Proxy(itemRef as object, {
    get(target, prop, receiver) {
      if (real !== undefined) return Reflect.get(real as object, prop, receiver);
      if (typeof prop === "symbol" || Object.hasOwn(target, prop)) return Reflect.get(target, prop, receiver);
      if (prop === "optional") return slotOptionalClosure(itemRef);
      if (prop === "with") return slotWithClosure(itemRef);
      return onFieldAccess(prop);
    },
  }) as SlotHandle;
  return {
    proxy,
    materialize(value: SlotHandle) {
      if (real !== undefined) throw new Error("lazyForEachItem: materialized more than once");
      real = value;
    },
  };
}
