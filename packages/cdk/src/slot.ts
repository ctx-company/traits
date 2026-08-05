import { dispatchSlotForEach } from "./functional/context.js";
import type { JsonObject, JsonValue, WriteOperation } from "./generated.js";
import type {
  DeclaredSlotHandle,
  DeclaredSlotWithFields,
  FieldRef,
  OutputSinkHandle,
  SequenceHandle,
  SlotHandle,
} from "./handles.js";
import { optionalSlot as optionalSlotInput } from "./input.js";
import { attachMeta, metaOf, withDeclaration, withHiddenField, withMeta } from "./meta.js";
import { collectMany, compact, validateSlug } from "./normalize.js";
import { refText } from "./ref.js";
import { schemaArray } from "./schema.js";
import type { SchemaValue } from "./schema.js";

export interface SlotFields {
  readonly id: string;
  readonly schema: SchemaValue;
  readonly description?: string;
  readonly hint?: string;
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
  <Value>(fields: SlotFields & { readonly schema: SchemaValue<Value>; }): DeclaredSlotWithFields<Value>;
  /** Text-schema slot shorthand; a bare string is shorthand for `{ id: value }`. @example `slot.text("summary")` */
  text(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<string>;
  /** Boolean-schema slot shorthand. @example `slot.boolean("approved")` */
  boolean(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<boolean>;
  /** Numeric-schema slot shorthand. @example `slot.number("retry-count")` */
  number(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<number>;
  /** Any-JSON-schema slot shorthand, for output shapes not worth declaring precisely. @example `slot.any("raw")` */
  any(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<JsonValue>;
  /** Curried list-slot factory: bind the element schema once, declare several slots of that list shape. @example `const findingsList = slot.array(schema.text()); const findings = findingsList("findings");` */
  array<Value>(
    schemaValue: SchemaValue<Value>,
  ): (value: string | Omit<SlotFields, "schema">) => DeclaredSlotHandle<Value[]>;
  /** List-slot shorthand with the element schema and id/fields both given. @example `slot.array(schema.text(), "findings")` */
  array<Value>(
    schemaValue: SchemaValue<Value>,
    value: string | Omit<SlotFields, "schema">,
  ): DeclaredSlotHandle<Value[]>;
  /** List-of-text-slot shorthand, the common case of `slot.array(schema.text(), ...)`. @example `slot.texts("notes")` */
  texts(value: string | Omit<SlotFields, "schema">): DeclaredSlotHandle<string[]>;
  /** Full-form slot declaration, alias of the bare `slot(...)` call for when a named property reads better than a call expression. @example `slot.of({ id: "review", schema: schema.text() })` */
  of<Value>(fields: SlotFields & { readonly schema: SchemaValue<Value>; }): DeclaredSlotWithFields<Value>;
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
  /** The `operation.over` write mode that appends to a list slot. */
  readonly Append: "append";
  /** The `operation.over` write mode that merges an object into an object slot. */
  readonly Merge: "merge";
  /** The `operation.over` write mode that replaces the slot's whole value (the default). */
  readonly Replace: "replace";
  /** The `operation.over` write mode that increments a numeric slot. */
  readonly Increment: "increment";
  /**
   * Builds the `set-field` write-mode value for `operation.over`: writes one
   * field of an object slot without touching the rest of it.
   * @example `operation.over(review, operation.SetField("status"))`
   */
  SetField(field: string): Extract<WriteOperation, { readonly "set-field": string; }>;
  /** Appends a step's output to a list slot instead of replacing it. @example `operation.over(findings, operation.Append)` */
  over<Value>(slotHandle: SlotHandle<Value[]>, operation: "append"): OutputSinkHandle<Value>;
  /** Increments a numeric slot by a step's numeric output. @example `operation.over(retryCount, operation.Increment)` */
  over(slotHandle: SlotHandle<number>, operation: "increment"): OutputSinkHandle<number>;
  /** Merges a step's object output into an object slot, or writes one field via `operation.SetField`. @example `operation.over(review, operation.Merge)` */
  over<Value>(
    slotHandle: SlotHandle<Value>,
    operation: "merge" | Extract<WriteOperation, { readonly "set-field": string; }>,
  ): OutputSinkHandle<JsonValue>;
  /** Replaces the slot's whole value (the default write mode — same as passing the slot directly as `output`). @example `operation.over(review, operation.Replace)` */
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
 * @see {@link operation.over}
 * @see {@link port}
 */
function slotFn<Value>(fields: SlotFields & { readonly schema: SchemaValue<Value>; }): DeclaredSlotWithFields<Value> {
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
export const slot: SlotFunction = Object.assign(
  slotFn,
  {
    text: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, "schema:text"),
    boolean: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, "schema:boolean"),
    number: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, "schema:number"),
    any: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, "schema:any"),
    array: (schemaValue: SchemaValue, value?: string | Omit<SlotFields, "schema">) =>
      value === undefined
        ? (slotValue: string | Omit<SlotFields, "schema">) => slotWithSchema(slotValue, schemaArray(schemaValue))
        : slotWithSchema(value, schemaArray(schemaValue)),
    texts: (value: string | Omit<SlotFields, "schema">) => slotWithSchema(value, schemaArray("schema:text")),
    of: slotFn,
  },
) as SlotFunction;

/**
 * Selects how a sequence step's output writes to a slot: replace it
 * (the default — plain `output: slot`), append to a list slot, merge into
 * an object slot, set one field, or increment a number slot.
 *
 * Without this, every step that accumulates into the same list or counter
 * slot would need a hand-written `{ slot, operation }` sink object, with no
 * check that `operation` is a value the runtime actually recognizes.
 * `operation.over(slot, operation.Append)` returns a typed `OutputSinkHandle`
 * usable directly as a sequence step's `output`.
 *
 * @param slotHandle The destination slot.
 * @example
 * ```ts
 * const findings = slot.array(schema.text(), "findings");
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
  operation: "merge" | Extract<WriteOperation, { readonly "set-field": string; }>,
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

function slotOf(fields: SlotFields): DeclaredSlotHandle {
  validateSlug(fields.id, "slot.id");
  const declaration = compact({
    id: fields.id,
    schema: refText(fields.schema, "slot.schema"),
    description: fields.description ?? `Runtime slot ${fields.id}.`,
    hint: fields.hint,
  });
  // The handle's own enumerable surface is empty (only field access, via the
  // proxy below for object schemas, is public) — the canonical `{ id,
  // schema, description, hint }` declaration lives only in `meta.declaration`.
  // See `withDeclaration` in `meta.ts` for the shared split every other
  // declaration builder (agent/port/prompt/schema.object/...) uses too.
  const handle = withDeclaration("slot", `slot:${fields.id}`, declaration, {} as JsonObject, {
    declarations: collectMany([fields.schema]),
  });
  const objectFields = objectSchemaFields(fields.schema);
  const declarations = collectMany([handle]);
  const resolved = objectFields === undefined
    ? handle as SlotHandle
    : fieldRefProxy(handle as SlotHandle, fields.id, `slot:${fields.id}`, [], objectFields, declarations) as SlotHandle;
  // `.optional()` is the per-SITE optionality wrapper, identical in output to
  // `input.optional(slot)` — optionality has never been a property of the slot
  // itself, so the same slot stays required at one step and optional at
  // another. Attached non-enumerably so it can never serialize into the
  // canonical, and defined here (not on the proxy path alone) so object-schema
  // and scalar slots both carry it.
  const withOptional = withHiddenField(resolved, "optional", () => optionalSlotInput(resolved));
  // `.forEach` is the functional layer's `items.forEach` spelling (0106,
  // 0102) — attached the same way as `.optional`, non-enumerable so it never
  // reaches the canonical declaration.
  return withHiddenField(
    withOptional,
    "forEach",
    (title: string, opts: unknown, body: (item: SlotHandle) => void) =>
      dispatchSlotForEach(withOptional, title, opts, body) as SequenceHandle,
  );
}

/** One declared object-schema field's canonical shape, as it appears in `declaration.fields`. */
interface SchemaFieldSummary {
  readonly schema: string;
}

/**
 * The declared fields of an object schema (`schema.object`/`extend`/
 * `template`), keyed by field id, or `undefined` when `schemaValue` isn't a
 * declared object schema (a built-in, list, union, or enum schema has no
 * per-field access).
 */
function objectSchemaFields(schemaValue: SchemaValue): Record<string, SchemaFieldSummary> | undefined {
  const fields = (metaOf(schemaValue)?.declaration as { readonly fields?: JsonObject; } | undefined)?.fields;
  return fields === undefined || fields === null || typeof fields !== "object" || Array.isArray(fields)
    ? undefined
    : fields as unknown as Record<string, SchemaFieldSummary>;
}

/**
 * The declared fields of the local object schema `ref` points at, looked up
 * in an already-collected declaration set — or `undefined` when `ref` isn't
 * a local schema ref, isn't declared, or declares no inline fields (a
 * resource-backed or scalar-enum schema has no per-field access, and a
 * field pointing at one is a recursion leaf).
 */
function objectSchemaFieldsByRef(
  ref: string,
  declarations: ReturnType<typeof collectMany>,
): Record<string, SchemaFieldSummary> | undefined {
  if (!ref.startsWith("schema:") || ref.includes(":", "schema:".length)) return undefined;
  const id = ref.slice("schema:".length);
  const declaration = declarations.schema?.find((candidate) => (candidate as { readonly id?: string; }).id === id) as
    | { readonly fields?: JsonObject; }
    | undefined;
  const fields = declaration?.fields;
  return fields === undefined || fields === null || typeof fields !== "object" || Array.isArray(fields)
    ? undefined
    : fields as unknown as Record<string, SchemaFieldSummary>;
}

/**
 * Wraps an object-schema slot handle (or, recursively, an object-schema
 * `FieldRef`) in a proxy exposing one `FieldRef` per declared field
 * (`slot.foo`, `slot["exit-code"]`) alongside the base value's normal
 * surface. When an accessed field's own declared schema is itself a local
 * object schema, the returned `FieldRef` recurses the same way, so
 * `slot.a.b.c` mints `fieldRef: { slotRef, field: "a.b.c" }` — task 0085's
 * dot-joined path encoding. Lists and scalar leaves stop the recursion.
 *
 * Uses own-field lookup — so a name like `constructor` cannot resolve
 * through `Object.prototype` — and throws, naming the slot and the full
 * dotted path, on any other unknown string access; symbol access (CDK
 * metadata, `Symbol.toPrimitive`, ...) always passes through untouched.
 *
 * Every minted `FieldRef`, at any depth, carries the same parent
 * `declarations` (the slot declaration plus every transitively-collected
 * schema declaration reachable from it), so `condition.equals(slot.a.b,
 * value)` collects exactly what a hand-written `{ slot, field: "a.b",
 * equals }` guard would — omitting this would let a trait authored through
 * field access silently drop the slot/schema declarations from the built
 * trait.
 */
function fieldRefProxy(
  base: object,
  slotId: string,
  slotRef: string,
  fieldPath: readonly string[],
  fields: Record<string, SchemaFieldSummary>,
  declarations: ReturnType<typeof collectMany>,
): SlotHandle | FieldRef {
  const fieldRefCache = new Map<string, FieldRef>();
  return new Proxy(base, {
    get(target, prop, receiver) {
      if (typeof prop === "symbol" || Object.hasOwn(target, prop)) return Reflect.get(target, prop, receiver);
      const field = Object.hasOwn(fields, prop) ? fields[prop] : undefined;
      if (field === undefined) {
        const fullPath = [...fieldPath, prop].join(".");
        throw new Error(`slot ${JSON.stringify(slotId)} has no field ${JSON.stringify(fullPath)}`);
      }
      let fieldRef = fieldRefCache.get(prop);
      if (fieldRef === undefined) {
        const nextPath = [...fieldPath, prop];
        const leaf = attachMeta({}, { fieldRef: { slotRef, field: nextPath.join(".") }, declarations });
        const nestedFields = objectSchemaFieldsByRef(field.schema, declarations);
        fieldRef = nestedFields === undefined
          ? leaf
          : fieldRefProxy(leaf, slotId, slotRef, nextPath, nestedFields, declarations) as FieldRef;
        fieldRefCache.set(prop, fieldRef);
      }
      return fieldRef;
    },
  }) as SlotHandle | FieldRef;
}
