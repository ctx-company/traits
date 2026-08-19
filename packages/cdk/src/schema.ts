import type { SchemaValue } from "./authoring-types.js";
import { schemaForms } from "./generated.js";
import type { JsonObject, JsonValue, SchemaBuiltin } from "./generated.js";
import type { SchemaHandle, SchemaRef } from "./handles.js";
import { attachMeta, metaOf, withDeclaration, withMeta } from "./meta.js";
import type { CdkObject } from "./meta.js";
import { collectMany, compact, stableObject, validateSlug } from "./normalize.js";
import { ref, refText } from "./ref.js";

export type { SchemaValue } from "./authoring-types.js";

/**
 * One item of a model-produced checklist (`schema.checklist()`). The shape
 * is closed and validated in Rust (`modules/core/src/trait/checklist.rs`),
 * not here — this type documents that shape, it does not enforce it.
 */
export type ChecklistItem = {
  /** Stable id, minted by the model at production time. */
  readonly id: string;
  /** The item's criterion or task text. */
  readonly text: string;
  /** Optional supporting detail. */
  readonly detail?: string;
  /** One of `"todo" | "done" | "waived"`. */
  readonly status: "todo" | "done" | "waived";
  /** Optional supporting evidence. */
  readonly evidence?: string;
};
export type SchemaUnionHandle<Value = unknown> = Pick<string, keyof string> & SchemaHandle<Value>;
type EnumLiteral = string | number | boolean;
export type SchemaEnumSpec<T extends readonly EnumLiteral[] = readonly EnumLiteral[]> = CdkObject & {
  readonly schema: string;
  readonly allowed: T;
  readonly __ctxTraitHandleKind?: "schema-enum";
};

/**
 * `Value` defaults to `unknown` so plain `SchemaFieldFields` (no type
 * argument) still means "some field, represented value not tracked" at
 * every existing untyped use site. `schema.field`/`schema.optional` are
 * generic over `Value` (see below) so a *typed* field — `schema.field(schema
 * .text())`, `schema.optional(schema.text())` — carries `Value` through
 * `schema: SchemaValue<Value> | SchemaEnumSpec`, which is what
 * `InferredFieldValue` (below) reads back out. Without a `Value` parameter
 * here, that read-back always lands on the untyped `SchemaValue | SchemaEnumSpec`
 * union's default `unknown`, even once the field's own producing call is
 * generic — the type information has nowhere left to live.
 */
export interface SchemaFieldFields<Value = unknown> {
  readonly schema: SchemaValue<Value> | SchemaEnumSpec;
  readonly required?: boolean;
  readonly description?: string;
  readonly hint?: string;
  readonly allowed?: readonly EnumLiteral[];
}

export type SchemaObjectField = SchemaValue | SchemaEnumSpec | SchemaFieldFields;
export type SchemaObjectFields = Record<string, SchemaObjectField>;

/**
 * One `schema.object` field's normalized canonical JSON: what `schema.field`/
 * a bare schema value/an already-declared field lower to, and what an
 * object-schema handle's own enumerable surface holds one of per declared
 * field (see `SchemaObjectHandle`).
 */
export interface SchemaFieldRecord {
  readonly schema: string;
  readonly required?: boolean;
  readonly description?: string;
  readonly hint?: string;
  readonly allowed?: readonly EnumLiteral[];
}

/**
 * A declared `schema.object`/`schema.extend`/`schema.template` handle: its
 * own enumerable surface is exactly its normalized field records (keyed by
 * field id), so `{ ...objectSchema }` composes with plain JavaScript spread
 * — the canonical `{ id, fields, description }` declaration itself lives
 * only in `Meta.declaration` (see `withDeclaration` in `meta.ts`).
 */
export type SchemaObjectHandle<Value = unknown> = SchemaHandle<Value> & {
  readonly [fieldId: string]: SchemaFieldRecord;
};

// Reads `Value` straight off `SchemaFieldFields<Value>`'s own type parameter
// instead of re-deriving it from `Field["schema"]`'s declared (parameter-
// erased) union type — `SchemaFieldFields<Value>.schema` is typed as
// `SchemaValue<Value> | SchemaEnumSpec` (the latter unparameterized, since a
// field can genuinely wrap either), and re-inferring through that union
// would let the untyped `SchemaEnumSpec` arm's default literal type leak
// into every field's inferred value, enum or not.
type InferredFieldValue<Field> = Field extends SchemaFieldFields<infer Value> ? Value : InferredSchemaValue<Field>;
type InferredSchemaValue<Value> =
  Value extends SchemaEnumSpec<infer Literals>
    ? Literals[number]
    : Value extends SchemaRef<infer Inner>
      ? Inner
      : Value extends SchemaHandle<infer Inner>
        ? Inner
        : unknown;
type FieldIsOptional<Field> = Field extends SchemaFieldFields
  ? Field["required"] extends false
    ? true
    : false
  : false;

/**
 * Infers an object schema's represented value shape from the
 * `SchemaObjectFields` map passed to `schema.object`/`schema.extend`/
 * `schema.template`'s `build`: a field with `required: false` (via
 * `schema.optional`) becomes an optional property, everything else stays
 * required — the same author-time inference `slot`/`port` already carry for
 * built-in schema values, extended to declared object shapes.
 */
export type SchemaObjectValue<Fields extends SchemaObjectFields> = {
  [K in keyof Fields as FieldIsOptional<Fields[K]> extends true ? never : K]: InferredFieldValue<Fields[K]>;
} & { [K in keyof Fields as FieldIsOptional<Fields[K]> extends true ? K : never]?: InferredFieldValue<Fields[K]> };

declare const SPOT_ORIGIN: unique symbol;
/**
 * Phantom, type-only marker (never assigned at runtime — `SPOT_ORIGIN` is
 * declared, not defined) threading a `schema.template` spot's *own* name
 * through whatever `build` returns it as: assigned bare (`value:
 * spots.item`) or wrapped (`value: schema.field(spots.item)`). `build`'s
 * `spots` parameter is `TaggedSpots<Spots>`, so every spot value already
 * carries its own name as this brand; `schema.field` (below) forwards the
 * brand from its input onto the field record it returns. `ResolvedFields`
 * reads the brand back off each built field to know which spot it actually
 * came from — instead of assuming a field's *own* key equals its spot's
 * name, which breaks the moment `build` renames a spot into a differently
 * named field (see `SchemaTemplateFunction`).
 */
type SpotOrigin<Name extends PropertyKey> = { readonly [SPOT_ORIGIN]?: Name };
/** `build`'s `spots` argument type: every spot value tagged with its own key. */
type TaggedSpots<Spots extends Record<string, SchemaValue>> = {
  readonly [K in keyof Spots]: Spots[K] & SpotOrigin<K>;
};
/**
 * Reads a built field's spot origin back out: a bare spot value carries the
 * brand directly, a `schema.field(...)`-wrapped one carries it on its own
 * `.schema` — the same two shapes `OriginOf`'s caller (`ResolvedField`)
 * already has to distinguish when substituting an override.
 */
type OriginOf<Field> =
  Field extends SpotOrigin<infer Name>
    ? Name
    : Field extends { readonly schema: infer FieldSchema }
      ? FieldSchema extends SpotOrigin<infer Name>
        ? Name
        : never
      : never;

/**
 * Resolves one built `Fields[K]` entry against `Overrides`, keyed by the
 * field's *spot* origin (via `OriginOf`) rather than the field's own key —
 * so a field renamed away from its spot's name (`value: schema.field(spots
 * .item)`) still specializes correctly. An overridden field keeps its
 * original metadata (`required`, `description`, ...) but swaps in the
 * override's value type; a field with no matching spot override keeps its
 * original `Fields[K]` type unchanged.
 */
type ResolvedField<Field extends SchemaObjectField, Overrides> = ApplyOverride<
  Field,
  Overrides[OriginOf<Field> & keyof Overrides]
>;
type ApplyOverride<Field extends SchemaObjectField, Override> = Override extends SchemaValue
  ? Field extends SchemaFieldFields
    ? Omit<Field, "schema"> & { readonly schema: Override }
    : Override
  : Field;

/**
 * Per-specialization field shape for `schema.template(...)`: each built
 * field whose *spot origin* (not field key — see `OriginOf`) matches an
 * override takes that override's value type; every other field keeps the
 * shape inferred from the template's default `build` call. This lets
 * `schema.template` preserve per-spot identity across calls (see
 * `SchemaTemplateFunction`) instead of every specialization sharing one
 * fixed `Fields` type regardless of what was overridden.
 */
export type ResolvedFields<Fields extends SchemaObjectFields, Overrides> = {
  [K in keyof Fields]: ResolvedField<Fields[K], Overrides>;
};

/**
 * The specialization function `schema.template(...)` returns: give it a
 * fresh schema id and (optionally) override some of the template's named
 * spots, get back an ordinary declared object schema. Each spot stays
 * addressable by its own name (`spots.value`, `spots.note`, ...) rather than
 * matched structurally, so two spots that happen to share a default shape
 * still specialize independently — including at the type level: a call that
 * overrides `value` with `schema.number()` types that specialization's
 * `value` field as `FieldRef<number>`, while other specializations (and
 * `value`'s sibling fields) keep the template's default-spot types.
 */
export interface SchemaTemplateFunction<Spots extends Record<string, SchemaValue>, Fields extends SchemaObjectFields> {
  <Overrides extends { readonly [K in keyof Spots]?: SchemaValue } = Record<string, never>>(
    schemaId: string,
    overrides?: Overrides,
  ): SchemaObjectHandle<SchemaObjectValue<ResolvedFields<Fields, Overrides>>>;
}

/**
 * Schema builder call signatures: the shape a port, slot, or object field
 * carries, checked structurally instead of left as an untyped JSON blob.
 *
 * Writing a schema ref by hand (`"schema:text"`, or a raw `{ type: "object",
 * properties: {...} }`) risks a field name or type mismatch between the
 * schema and the code that reads the resulting value, caught only when a
 * model's structured output fails to parse at run time. `schema.*` builders
 * return typed refs/handles instead of a hand-assembled blob, and the inline
 * built-ins (`schema.text()`, `.number()`, ...) carry a real TS type
 * parameter (`SchemaRef<string>`, `SchemaRef<number>`, ...) that flows into
 * `slot`/`port`'s inferred value type. `schema.object`, once declared,
 * carries its field shape through `SchemaObjectHandle<SchemaObjectValue<
 * Fields>>`; the declared enum-family forms (`schema.enum`/`schema.oneOf`/
 * `schema.verdict`/`schema.decision`/`schema.yesNo`) likewise carry their
 * literal vocabulary through `SchemaHandle<Value>` — `rating`'s bounds
 * aren't statically known, so its declared form narrows only to `number`.
 * Whether a schema ref actually resolves, and whether a value matches it,
 * is validated by the Rust synth/runtime, not `tsc`.
 *
 * `schema.text()`/`boolean()`/`number()`/`integer()`/`any()` are inline
 * built-in refs (no id, nothing to declare). `schema.object`/`schema.enum`
 * (with an id) are named declarations other schemas and ports/slots can
 * reference by ref. `schema.list`/`array` wrap an inner schema as a
 * collection. `schema.union` composes several schema refs into one.
 * `schema.field` wraps a schema as one object field with
 * required/description/hint metadata. `schema.zod`/`typebox` adapt an
 * existing Zod/TypeBox schema instead of re-declaring it in CDK form.
 *
 * @param inner The nested schema for list and array forms.
 * @example `schema.list(schema.text())`
 * @see {@link SchemaValue}
 */
export interface SchemaFunction {
  /** Inline string schema ref, typed as `SchemaRef<string>`. @example `port.output.of(schema.text(), { id: "summary" })` */
  text(): "schema:text" & SchemaRef<string>;
  /** Inline boolean schema ref, typed as `SchemaRef<boolean>`. @example `port.output.of(schema.boolean(), { id: "approved" })` */
  boolean(): "schema:boolean" & SchemaRef<boolean>;
  /** Inline numeric (float-permitting) schema ref, typed as `SchemaRef<number>`. @example `port.output.of(schema.number(), { id: "confidence" })` */
  number(): "schema:number" & SchemaRef<number>;
  /** Inline integer schema ref, typed as `SchemaRef<number>`. @example `port.output.of(schema.integer(), { id: "count" })` */
  integer(): "schema:integer" & SchemaRef<number>;
  /** Inline schema ref that accepts any JSON value — the escape hatch for output shapes not worth declaring precisely. @example `port.output.of(schema.any(), { id: "raw" })` */
  any(): "schema:any" & SchemaRef<JsonValue>;
  /**
   * A model-produced checklist: a list of {@link ChecklistItem}, coverage-
   * checked in Rust against the latest accepted replace write to the same
   * slot. Use `condition.count(slot).where("status", "done")` for a loop
   * exit guard. @example `slot("plan", schema.checklist())`
   */
  checklist(): SchemaRef<ChecklistItem[]>;
  /**
   * Wraps `inner` as a list schema ref. Declaring it by hand as
   * `{ type: "array", items: x }` risks the items ref not being collected as
   * a declaration; `schema.list` collects it automatically.
   * @example `schema.list(schema.text())`
   */
  list<Value>(inner: SchemaValue<Value>): SchemaRef<Value[]>;
  /** Composes several schema refs into one union ref. @example `schema.union([schema.text(), schema.list(schema.text())])` */
  union<const Members extends readonly SchemaValue[]>(
    members: Members,
  ): SchemaUnionHandle<InferredSchemaValue<Members[number]>>;
  /**
   * Creates an inline (no id) or declared (with id) closed-vocabulary enum
   * schema. `schema.oneOf`/`schema.verdict`/`schema.decision`/`schema.yesNo`
   * (the CDK's `index.ts` common shapes) are thin wrappers over this for the
   * vocabularies that recur across traits.
   * @example `schema.enum(["pass", "fail", "needs-changes"])`
   */
  enum<const T extends readonly EnumLiteral[]>(values: T): SchemaEnumSpec<T>;
  enum<const T extends readonly EnumLiteral[]>(
    id: string,
    values: T,
    fields?: { readonly description?: string },
  ): SchemaHandle<T[number]>;
  /**
   * Declares a named object schema from a map of field id to field schema.
   * @example
   * ```ts
   * schema.object("code-review-finding", {
   *   severity: schema.field(schema.oneOf("severity", ["blocker", "advisory"])),
   *   file: schema.text(),
   *   summary: schema.text(),
   * });
   * ```
   */
  object<Fields extends SchemaObjectFields>(
    id: string,
    fields: Fields,
    options?: { readonly description?: string },
  ): SchemaObjectHandle<SchemaObjectValue<Fields>>;
  /**
   * Wraps a schema as one `schema.object` field, attaching
   * required/description/hint metadata a bare schema ref can't carry.
   * @example `schema.field(schema.text(), { description: "The finding's file." })`
   */
  field<FieldSchema extends SchemaValue | SchemaEnumSpec>(
    schemaValue: FieldSchema,
    fields?: Omit<SchemaFieldFields, "schema">,
  ): SchemaFieldFields<InferredSchemaValue<FieldSchema>> &
    Pick<FieldSchema, Extract<keyof FieldSchema, typeof SPOT_ORIGIN>>;
  /**
   * Combines several already-normalized field-record maps (plain
   * `SchemaObjectFields`, or another `schema.object`/`extend`/`template`
   * handle spread as one) plus an optional trailing map of additional
   * fields, checking every incoming field id for a collision instead of the
   * silent last-wins a plain `{ ...a, ...b }` spread gives you.
   * @example `schema.object("finding-with-owner", schema.extend(finding, { owner: schema.text() }))`
   */
  extend(...sources: readonly (SchemaObjectFields | SchemaObjectHandle)[]): SchemaObjectFields;
  /**
   * Marks an existing field value/field record as optional (`required:
   * false`) without inventing a second field representation — the same
   * normalized shape `schema.field(...)` produces, just with `required`
   * forced to `false`.
   * @example `schema.object("finding", { note: schema.optional(schema.text()) })`
   */
  optional<Field extends SchemaObjectField>(
    fieldValue: Field,
  ): SchemaFieldFields<InferredFieldValue<Field>> & { readonly required: false };
  /**
   * A monomorphizing factory over `schema.object`: declares several named
   * "spots" with default schemas, and returns a specialization function that
   * builds one concrete object schema per call, overriding some spots and
   * keeping the rest at their default. Each spot is threaded through by
   * name (never matched by shape), so two spots sharing a default shape
   * still specialize independently.
   * @example
   * ```ts
   * const note = schema.template("note", { value: schema.text() }, (spots) => ({
   *   value: schema.field(spots.value),
   * }));
   * const textNote = note("text-note");
   * const numberNote = note("number-note", { value: schema.number() });
   * ```
   */
  template<Spots extends Record<string, SchemaValue>, Fields extends SchemaObjectFields>(
    templateId: string,
    defaultSpots: Spots,
    build: (spots: TaggedSpots<Spots>) => Fields,
  ): SchemaTemplateFunction<Spots, Fields>;
  /**
   * Declares a `schema.object`/`schema.enum` from an existing Zod schema
   * instead of re-declaring the same object shape twice (once for the
   * model's own validation, once for the CDK trait) — pass your own
   * `toJsonSchema` adapter (e.g. `zod-to-json-schema`) since the CDK does
   * not depend on Zod itself. Only object/scalar/scalar-array/enum shapes
   * are supported; unsupported JSON Schema keywords throw at build time.
   * @example
   * ```ts
   * schema.zod("result", z.object({ name: z.string(), enabled: z.boolean() }), {
   *   toJsonSchema: (value) => zodToJsonSchema(value as z.ZodTypeAny),
   * });
   * ```
   */
  zod(id: string, source: unknown, options: { readonly toJsonSchema: (source: unknown) => unknown }): SchemaHandle;
  /**
   * Declares a `schema.object`/`schema.enum` from an existing TypeBox (or
   * hand-written) JSON Schema value, same supported-shape constraints as
   * `schema.zod`.
   * @example
   * ```ts
   * schema.typebox("typebox-result", {
   *   type: "object",
   *   required: ["name"],
   *   properties: { name: { type: "string" } },
   * });
   * ```
   */
  typebox(id: string, jsonSchema: unknown): SchemaHandle;
  /** Stable, guard-visible vocabulary: `pass|fail|needs-changes`. */
  verdict(id?: string): SchemaHandle<(typeof VERDICT_VALUES)[number]>;
  /** Stable workflow vocabulary: `approved|revise`. */
  decision(id?: string): SchemaHandle<(typeof DECISION_VALUES)[number]>;
  /** Stable binary vocabulary: `yes|no`. */
  yesNo(id?: string): SchemaHandle<(typeof YES_NO_VALUES)[number]>;
  /** Creates an inline enum schema specification from values in their supplied order. */
  oneOf<const Values extends readonly EnumLiteral[]>(...values: Values): SchemaEnumSpec<Values>;
  /** Creates a declared enum schema from an explicit ID and value tuple. */
  oneOf<const Values extends readonly EnumLiteral[]>(id: string, values: Values): SchemaHandle<Values[number]>;
  /** Creates an inline enum schema specification for an inclusive integer range. */
  rating(min: number, max: number): SchemaEnumSpec<readonly number[]>;
  /** Creates a declared enum schema for an inclusive integer range. */
  rating(id: string, min: number, max: number): SchemaHandle<number>;
}

const VERDICT_VALUES = ["pass", "fail", "needs-changes"] as const;
const DECISION_VALUES = ["approved", "revise"] as const;
const YES_NO_VALUES = ["yes", "no"] as const;

function schemaOneOf<const Values extends readonly EnumLiteral[]>(...values: Values): SchemaEnumSpec<Values>;
function schemaOneOf<const Values extends readonly EnumLiteral[]>(
  id: string,
  values: Values,
): SchemaHandle<Values[number]>;
// The two overloads above are irreducibly shape-ambiguous at the
// implementation boundary: `(id: string, values: Values)` (declared form) vs
// `(...values: Values)` where `Values[number]` may itself be a string
// (inline form, e.g. `schema.oneOf("pass", "fail")`) — only the arg COUNT
// plus an `Array.isArray` check on the second position (not the static
// param types) can tell them apart, so `args` stays `readonly unknown[]`.
function schemaOneOf(...args: readonly unknown[]): SchemaEnumSpec | SchemaHandle {
  if (args.length === 2 && typeof args[0] === "string" && Array.isArray(args[1])) {
    return schemaEnum(args[0], args[1] as readonly EnumLiteral[]);
  }
  return schemaEnum(args as readonly EnumLiteral[]);
}

function ratingValues(min: number, max: number): number[] {
  if (!Number.isInteger(min) || !Number.isInteger(max)) {
    throw new Error("schema.rating: bounds must be integers");
  }
  if (min > max) throw new Error("schema.rating: min must not exceed max");
  return Array.from({ length: max - min + 1 }, (_, index) => min + index);
}

function schemaRating(min: number, max: number): SchemaEnumSpec<readonly number[]>;
function schemaRating(id: string, min: number, max: number): SchemaHandle<number>;
function schemaRating(
  idOrMin: string | number,
  minOrMax: number,
  max?: number,
): SchemaEnumSpec<readonly number[]> | SchemaHandle {
  // oxlint-disable-next-line typescript/no-non-null-assertion -- the two-overload contract guarantees `max` is supplied when `idOrMin` is a string
  if (typeof idOrMin === "string") return schemaEnum(idOrMin, ratingValues(minOrMax, max!));
  return schemaEnum(ratingValues(idOrMin, minOrMax));
}

function schemaEnum<const T extends readonly EnumLiteral[]>(values: T): SchemaEnumSpec<T>;
function schemaEnum<const T extends readonly EnumLiteral[]>(
  id: string,
  values: T,
  fields?: { readonly description?: string },
): SchemaHandle<T[number]>;
function schemaEnum(
  idOrValues: string | readonly EnumLiteral[],
  values?: readonly EnumLiteral[],
  fields?: { readonly description?: string },
): SchemaEnumSpec | SchemaHandle {
  // The implementation signature's `values`/`fields` are optional only to
  // fit the single-arg overload above; when `idOrValues` is a string, the
  // second-overload caller has always supplied `values` — the non-null
  // assertion below is the resulting single documented gap, not a cast.
  return typeof idOrValues === "string"
    ? // oxlint-disable-next-line typescript/no-non-null-assertion -- see the comment above: values is guaranteed by the overload contract
      schemaEnumDeclaration(idOrValues, values!, fields)
    : schemaEnumSpec(idOrValues);
}

const schemaBuiltins = {
  text: "schema:text",
  boolean: "schema:boolean",
  number: "schema:number",
  integer: "schema:integer",
  any: "schema:any",
  checklistItem: "schema:checklist-item",
} as const satisfies Record<string, SchemaBuiltin>;

/**
 * Creates built-in, collection, enum, and object schema declarations.
 * @param id A declared schema identifier where required.
 * @example `schema.object("result", { title: schema.text() })`
 * @see {@link slot}
 */
export const schema: SchemaFunction = {
  text: () => schemaBuiltins.text,
  boolean: () => schemaBuiltins.boolean,
  number: () => schemaBuiltins.number,
  integer: () => schemaBuiltins.integer,
  any: () => schemaBuiltins.any,
  checklist: () => schemaList(schemaBuiltins.checklistItem as unknown as SchemaValue<ChecklistItem>),
  list: schemaList,
  union: schemaUnion,
  enum: schemaEnum,
  // These are generic *function expressions* (not plain arrow wrappers) so
  // `schema.object(...)`/`schema.field(...)`/`schema.optional(...)`/
  // `schema.template(...)` still infer a fresh `Fields`/`Value` per call
  // site — a non-generic wrapper assigned to a generic interface method
  // collapses to one contextual instantiation and every caller would see
  // the same (widened) field shape.
  object: function object<Fields extends SchemaObjectFields>(
    id: string,
    fields: Fields,
    options: { readonly description?: string } = {},
  ) {
    return schemaObject(id, fields, options);
  },
  field: function field<FieldSchema extends SchemaValue | SchemaEnumSpec>(
    schemaValue: FieldSchema,
    fields: Omit<SchemaFieldFields, "schema"> = {},
  ) {
    return { ...fields, schema: schemaValue } as SchemaFieldFields<InferredSchemaValue<FieldSchema>> &
      Pick<FieldSchema, Extract<keyof FieldSchema, typeof SPOT_ORIGIN>>;
  },
  extend: (...sources) => schemaExtend(sources),
  optional: function optional<Field extends SchemaObjectField>(fieldValue: Field) {
    return schemaOptional(fieldValue);
  },
  template: function template<Spots extends Record<string, SchemaValue>, Fields extends SchemaObjectFields>(
    templateId: string,
    defaultSpots: Spots,
    build: (spots: TaggedSpots<Spots>) => Fields,
  ) {
    return schemaTemplate(templateId, defaultSpots, build);
  },
  zod: (id, source, options) => {
    rejectZodEffects(source, "schema.zod");
    return schemaFromJsonSchema(id, options.toJsonSchema(source), "schema.zod");
  },
  typebox: (id, jsonSchema) => schemaFromJsonSchema(id, jsonSchema, "schema.typebox"),
  verdict: (id = "verdict") => schemaEnumDeclaration(id, VERDICT_VALUES),
  decision: (id = "decision") => schemaEnumDeclaration(id, DECISION_VALUES),
  yesNo: (id = "yes-no") => schemaEnumDeclaration(id, YES_NO_VALUES),
  oneOf: schemaOneOf,
  rating: schemaRating,
};

/**
 * Declares a `[schema.*]` entry backed by an external resource file (a
 * standalone JSON Schema document) rather than an inline `schema.object`/
 * `schema.enum` declaration — for schemas maintained outside the trait
 * source, referenced by the resource that holds them.
 * @param id Schema identifier.
 * @param resourceId The backing resource's id.
 * @example `ctxSchemaResource("external-shape", "external-shape-json-schema")`
 * @see {@link schema}
 */
export function ctxSchemaResource(id: string, resourceId: string, fields: JsonObject = {}): JsonObject {
  return compact({ id, resource: ref("resource", resourceId), ...fields });
}

function schemaEnumSpec<T extends readonly EnumLiteral[]>(values: T): SchemaEnumSpec<T> {
  const schemaRef = enumSchemaRef(values, "schema.enum");
  // `Array.from` always returns a mutable array; `T` may be a readonly tuple subtype of
  // `readonly EnumLiteral[]`, which the checker can't confirm generically — the unknown detour
  // documents that gap rather than a false claim of structural overlap.
  const allowed = Array.from(values) as unknown as T; // audited-unknown-cast: mutable array -> generic readonly T, see comment above
  return withMeta({ schema: schemaRef, allowed }, { kind: "schema-field" });
}

function schemaEnumDeclaration<const T extends readonly EnumLiteral[]>(
  id: string,
  values: T,
  fields: { readonly description?: string } = {},
): SchemaHandle<T[number]> {
  validateSlug(id, "schema.id");
  const declaration = compact({
    id,
    schema: enumSchemaRef(values, `schema.${id}.enum`),
    allowed: Array.from(values),
    description: fields.description,
  });
  return withDeclaration("schema", `schema:${id}`, declaration, {});
}

function schemaObject<Fields extends SchemaObjectFields>(
  id: string,
  fields: Fields,
  options: { readonly description?: string },
): SchemaObjectHandle<SchemaObjectValue<Fields>> {
  validateSlug(id, "schema.id");
  const normalizedFields = normalizedFieldRecords(fields, `schema.${id}.fields`);
  // Two distinct top-level containers over the same per-field record
  // objects: `declaration.fields` is what's emitted as canonical JSON,
  // `publicSurface` is the handle's own enumerable surface (see
  // `withDeclaration` in meta.ts). They must not be the *same* object —
  // otherwise the schema's own top-level declaration meta, attached to the
  // handle/publicSurface, would also land on `declaration.fields`, and a
  // trait walking `declaration` would collect the whole schema declaration
  // a second time from inside its own `fields` map.
  const declaration = compact({ id, fields: { ...normalizedFields }, description: options.description });
  const declarations = collectMany(Object.values(normalizedFields));
  return withDeclaration<Record<string, SchemaFieldRecord>, "schema", SchemaObjectValue<Fields>>(
    "schema",
    `schema:${id}`,
    declaration,
    { ...normalizedFields },
    { declarations },
  );
}

/**
 * Normalizes a `SchemaObjectFields` map into per-field `SchemaFieldRecord`s,
 * each carrying its own non-enumerable `Meta.declarations` pointing at any
 * nested declared schema the field references — so plain-JS-spreading a
 * field record out of one object schema into another (`schema.extend`, or a
 * bare `{ ...a, ...b }`) still lets the destination schema's own
 * declaration-collection reach the field's nested schema, instead of only
 * seeing the field's already-lowered `schema: "schema:foo"` ref string.
 */
function normalizedFieldRecords(fields: SchemaObjectFields, fieldPath: string): Record<string, SchemaFieldRecord> {
  const normalized: Record<string, SchemaFieldRecord> = {};
  for (const [fieldId, field] of Object.entries(fields)) {
    validateSlug(fieldId, `${fieldPath}.${fieldId}`);
    const record = schemaFieldDeclaration(field, `${fieldPath}.${fieldId}`);
    const sources = schemaFieldDeclarationSources(field);
    normalized[fieldId] = attachMeta(record, {
      declarations: collectMany(sources),
    });
  }
  return normalized;
}

/**
 * Combines several field-record maps (a plain `SchemaObjectFields`, or
 * another declared object-schema handle spread as one), throwing a
 * field-specific collision error naming both sources when the same field id
 * is declared in more than one input — unlike a plain `{ ...a, ...b }`
 * spread, which silently keeps the last one.
 */
function schemaExtend(sources: readonly (SchemaObjectFields | SchemaObjectHandle)[]): SchemaObjectFields {
  const result: Record<string, SchemaObjectField> = {};
  const originOf = new Map<string, number>();
  sources.forEach((source, sourceIndex) => {
    for (const [fieldId, field] of Object.entries(source as SchemaObjectFields)) {
      const previousSource = originOf.get(fieldId);
      if (previousSource !== undefined) {
        throw new Error(
          `schema.extend: field ${JSON.stringify(
            fieldId,
          )} is declared by both source ${previousSource} and source ${sourceIndex}`,
        );
      }
      originOf.set(fieldId, sourceIndex);
      result[fieldId] = field;
    }
  });
  return normalizedFieldRecords(result, "schema.extend");
}

/**
 * Marks a field value/field record optional (`required: false`) without
 * inventing a second field representation. When `fieldValue` is already a
 * normalized field record spread out of another object-schema handle (e.g.
 * `schema.optional(base.inner)`), it carries its own `Meta.declarations`
 * pointing at whatever nested schema it references (see
 * `normalizedFieldRecords`) — preserve that on the returned optional record
 * instead of dropping it, or a spread field wrapped in `schema.optional`
 * would silently stop collecting its nested declaration.
 */
function schemaOptional<Field extends SchemaObjectField>(
  fieldValue: Field,
): SchemaFieldFields<InferredFieldValue<Field>> & { readonly required: false } {
  // `Result`'s `InferredFieldValue<Field>` is a purely compile-time read-back
  // of `Field` — the runtime object below is the same plain field record
  // regardless of what value type it's inferred to carry, so both branches
  // mint the phantom once at their own return.
  type Result = SchemaFieldFields<InferredFieldValue<Field>> & { readonly required: false };
  if (metaOf(fieldValue)?.ref !== undefined || typeof fieldValue === "string") {
    return { schema: fieldValue as SchemaValue, required: false } as Result;
  }
  const optionalField = { ...(fieldValue as SchemaFieldFields | SchemaEnumSpec), required: false as const };
  const declarations = metaOf(fieldValue)?.declarations;
  return (declarations === undefined
    ? optionalField
    : attachMeta(optionalField, { declarations })) as unknown as Result; // audited-unknown-cast: phantom-mint over inferred generic, see comment above
}

/**
 * A monomorphizing factory over `schema.object`: resolves each named spot
 * (an override if supplied, else the template's default) by name — never by
 * matching schema shape — calls `build` with the resolved spots to get the
 * specialization's fields, then declares an ordinary `schema.object`.
 * Records `{ templateId, params }` on the produced schema's `Meta.templateInfo`
 * for author-time provenance only; it never reaches draft/canonical JSON.
 */
function schemaTemplate<Spots extends Record<string, SchemaValue>, Fields extends SchemaObjectFields>(
  templateId: string,
  defaultSpots: Spots,
  build: (spots: TaggedSpots<Spots>) => Fields,
): SchemaTemplateFunction<Spots, Fields> {
  validateSlug(templateId, "schema.template.id");
  function specialize<Overrides extends { readonly [K in keyof Spots]?: SchemaValue } = Record<string, never>>(
    schemaId: string,
    overrides?: Overrides,
  ): SchemaObjectHandle<SchemaObjectValue<ResolvedFields<Fields, Overrides>>> {
    const resolvedSpots = { ...defaultSpots, ...overrides } as TaggedSpots<Spots>;
    const fields = build(resolvedSpots);
    const handle = schemaObject(schemaId, fields, {});
    const params: Record<string, string> = {};
    for (const [spotName, spotValue] of Object.entries(resolvedSpots)) {
      params[spotName] = refText(spotValue, `schema.template.${templateId}.${spotName}`);
    }
    const meta = metaOf(handle);
    attachMeta(handle, { ...meta, templateInfo: { templateId, params } });
    // `schemaObject` only ever builds the template's default `Fields` shape
    // — a per-spot `Overrides`-specialized return type is a compile-time-only
    // promise to callers (`ResolvedFields` reads back the same runtime
    // object under a narrower type), never a distinct runtime shape.
    return handle as SchemaObjectHandle<SchemaObjectValue<ResolvedFields<Fields, Overrides>>>;
  }
  return specialize;
}

/**
 * A boxed `String` wrapping a schema ref: the one JS value that both renders
 * as the ref text everywhere a primitive string is expected (template
 * interpolation, `JSON.stringify`, string comparison) AND can carry the
 * hidden `META` property `withMeta` attaches — a primitive `string` cannot
 * hold a non-enumerable property, only its boxed `String` wrapper can.
 * `Pick<string, keyof string>` documents that every `String.prototype`
 * member (`length`, `charAt`, ...) stays available on the box, same as on
 * the primitive it wraps; `schema.list`/`schema.union` are the only two
 * builders that need this (both mint a schema ref with declarations to
 * collect, rather than an already-declared handle).
 */
export type SchemaRefBox = Pick<string, keyof string> & CdkObject;

/** Boxes `schemaRef` per {@link SchemaRefBox}'s rationale. */
function boxSchemaRef(schemaRef: string): SchemaRefBox {
  return Object(schemaRef) as SchemaRefBox;
}

export function schemaList<Value>(value: SchemaValue<Value>): SchemaRef<Value[]> {
  const schemaRef = `${schemaForms.list.prefix}${refText(
    value,
    "schema.list",
  )}${schemaForms.list.suffix}` as SchemaRef<Value[]>;
  // Box the ref and carry the element's declarations, exactly as schema.union
  // does. A bare template string would render correctly and collect nothing:
  // an object schema reachable only through a list field would never be
  // declared, and the trait would fail validation naming a ref it does emit.
  const handle = boxSchemaRef(schemaRef);
  // `SchemaRef<V>`'s phantom rides on a `string &`-branded primitive, not
  // `withMeta`'s `Value`/`HANDLE_VALUE` mechanism (that only reaches
  // `Brand`-based `Handle<K, Value>` shapes) — a distinct phantom family, so
  // this crossing stays a survivor rather than folding into §3.4's mint.
  // `handle` is a boxed-string CdkObject at runtime (SchemaRefBox); `SchemaRef<V>`'s phantom rides on
  // the primitive `string` family instead, so the checker sees two unrelated types here — genuine gap.
  return withMeta(handle, {
    kind: "ref",
    ref: schemaRef,
    declarations: collectMany([value]),
  }) as unknown as SchemaRef<Value[]>; // audited-unknown-cast: boxed-string CdkObject -> branded string phantom, see comment above
}

/**
 * Splices any member that is itself an authored `schema.union(...)` into its
 * own (already-flattened) member list, in place and in order — so
 * `schema.union([schema.union([a, b]), c])` produces one flat
 * `(schema:a|schema:b|schema:c)` ref instead of nesting a union ref inside
 * a union ref. Detected via `Meta.unionMembers`, the pre-flatten member list
 * every `schema.union` handle carries privately (author-only, never
 * emitted) — not by pattern-matching the rendered ref text.
 */
function flattenUnionMembers(members: readonly SchemaValue[]): SchemaValue[] {
  const flattened: SchemaValue[] = [];
  for (const member of members) {
    const nested = metaOf(member)?.unionMembers;
    if (nested !== undefined) flattened.push(...flattenUnionMembers(nested));
    else flattened.push(member);
  }
  return flattened;
}

function schemaUnion<const Members extends readonly SchemaValue[]>(
  members: Members,
): SchemaUnionHandle<InferredSchemaValue<Members[number]>> {
  const flatMembers = flattenUnionMembers(members);
  if (flatMembers.length === 0) throw new Error("schema.union: members must not be empty");
  const refs = flatMembers.map((member, index) => refText(member, `schema.union[${index}]`));
  const seen = new Set<string>();
  for (const member of refs) {
    // A member is a plain schema ref or one list wrapper over a plain ref.
    const inner =
      member.startsWith(schemaForms.list.prefix) && member.endsWith(schemaForms.list.suffix)
        ? member.slice(schemaForms.list.prefix.length, member.length - schemaForms.list.suffix.length)
        : member;
    const [kind, path, ...extra] = inner.split(":");
    if (kind !== "schema" || path === undefined || extra.length !== 0) {
      throw new Error(`schema.union: member ${JSON.stringify(member)} must be a schema ref or a list of one`);
    }
    try {
      for (const segment of path.split("/")) validateSlug(segment, "schema.union member");
    } catch {
      throw new Error(`schema.union: member ${JSON.stringify(member)} must be a schema ref or a list of one`);
    }
    if (seen.has(member)) throw new Error(`schema.union: duplicate member ${JSON.stringify(member)}`);
    seen.add(member);
  }
  const schemaRef = `${schemaForms.union.prefix}${refs.join(
    schemaForms.union.separator,
  )}${schemaForms.union.suffix}` as SchemaRef;
  const handle = boxSchemaRef(schemaRef);
  // Same distinct-phantom-family note as `schemaList` above: `SchemaRef`'s
  // brand isn't `withMeta`'s `Value` mechanism.
  return withMeta(handle, {
    kind: "ref",
    ref: schemaRef,
    declarations: collectMany(flatMembers),
    unionMembers: flatMembers,
  }) as SchemaUnionHandle<InferredSchemaValue<Members[number]>>;
}

function schemaFieldDeclarationSources(field: SchemaObjectField): unknown[] {
  if (metaOf(field)?.ref !== undefined || typeof field === "string") return [field];
  // A field entry already normalized elsewhere (spread out of another
  // object-schema handle, or through `schema.extend`) carries its own
  // `Meta.declarations` pointing at whatever nested schema it references —
  // collect the field record itself rather than its now-lowered `.schema`
  // ref string, which by itself carries no declaration to collect.
  if (metaOf(field)?.declarations !== undefined) return [field];
  const schemaValue = (field as SchemaFieldFields | SchemaEnumSpec).schema;
  return schemaValue === undefined ? [] : [schemaValue];
}

function schemaFieldDeclaration(field: SchemaObjectField, fieldPath: string): SchemaFieldRecord {
  if (metaOf(field)?.ref !== undefined || typeof field === "string") {
    return { schema: refText(field as SchemaValue, `${fieldPath}.schema`), required: true };
  }
  const fieldObject = field as SchemaFieldFields | SchemaEnumSpec;
  const enumSpec = schemaEnumSpecValue(fieldObject.schema);
  const allowedValues = fieldObject.allowed ?? enumSpec?.allowed;
  // `compact` (not `compactAs`) here: its optional fields are built with
  // explicit `undefined` arms (`exactOptionalPropertyTypes` rejects those
  // against `Mutable<SchemaFieldRecord>`'s narrower optional-property
  // typing), and `compact`'s own `Object.entries(...).filter(item !==
  // undefined)` is exactly what erases them before the value is boxed as
  // `SchemaFieldRecord` below.
  return compact({
    schema: refText(enumSpec?.schema ?? fieldObject.schema, `${fieldPath}.schema`),
    // `SchemaEnumSpec` has no `required` field of its own — `fieldObject`
    // reads it through `CdkObject`'s index signature (`unknown`) on that
    // arm, same duck-typed union `compact` previously absorbed silently.
    required: (fieldObject as SchemaFieldFields).required ?? true,
    description: (fieldObject as SchemaFieldFields).description,
    hint: (fieldObject as SchemaFieldFields).hint,
    allowed: allowedValues === undefined ? undefined : normalizeAllowedValues(allowedValues, `${fieldPath}.allowed`),
  }) as unknown as SchemaFieldRecord; // audited-unknown-cast: JsonObject (explicit-undefined arms) -> closed field shape, see comment above
}

function schemaEnumSpecValue(value: unknown): SchemaEnumSpec | undefined {
  return metaOf(value)?.kind === "schema-field" ? (value as SchemaEnumSpec) : undefined;
}

function enumSchemaRef(
  values: readonly EnumLiteral[],
  fieldPath: string,
): "schema:text" | "schema:number" | "schema:boolean" {
  const allowed = normalizeAllowedValues(values, fieldPath);
  const firstType = typeof allowed[0];
  if (!allowed.every((value) => typeof value === firstType)) {
    throw new Error(`${fieldPath}: enum values must all have the same scalar type`);
  }
  if (firstType === "string") return "schema:text";
  if (firstType === "number") return "schema:number";
  if (firstType === "boolean") return "schema:boolean";
  throw new Error(`${fieldPath}: unsupported enum value type`);
}

function normalizeAllowedValues(values: readonly EnumLiteral[], fieldPath: string): EnumLiteral[] {
  if (values.length === 0) throw new Error(`${fieldPath}: enum values must not be empty`);
  const normalized = Array.from(values);
  const seen = new Set<string>();
  for (const value of normalized) {
    if (typeof value === "number" && !Number.isFinite(value)) {
      throw new Error(`${fieldPath}: enum values must be finite numbers`);
    }
    const key = `${typeof value}:${String(value)}`;
    if (seen.has(key)) throw new Error(`${fieldPath}: duplicate enum value ${JSON.stringify(value)}`);
    seen.add(key);
  }
  return normalized;
}

function assertEnumMatchesSchema(values: readonly EnumLiteral[], schemaValue: SchemaValue, fieldPath: string): void {
  const expectedType =
    schemaValue === "schema:text"
      ? "string"
      : schemaValue === "schema:number"
        ? "number"
        : schemaValue === "schema:integer"
          ? "number"
          : schemaValue === "schema:boolean"
            ? "boolean"
            : undefined;
  if (expectedType === undefined || values.some((value) => typeof value !== expectedType)) {
    throw new Error(`${fieldPath}: enum values must match the declared scalar type`);
  }
  if (
    schemaValue === "schema:integer" &&
    values.some((value) => typeof value !== "number" || !Number.isInteger(value))
  ) {
    throw new Error(`${fieldPath}: enum values must match the declared scalar type`);
  }
}

/**
 * `_def` property names, across Zod's wrapper types, that hold a nested Zod
 * schema (or an array of them) rather than a plain value: `type`/`innerType`
 * (array element, optional/nullable/default/branded/readonly/catch's wrapped
 * schema), `schema` (`ZodEffects`' wrapped schema), `left`/`right`
 * (intersection/pipe), `valueType`/`keyType` (record), `options` (union
 * members). Traversing every wrapper by this property-name convention,
 * instead of by an exhaustive `typeName` whitelist, means a wrapper kind this
 * list doesn't already know by name (e.g. `.brand()`'s `ZodBranded`, added to
 * Zod after this was first written) still gets walked into — the property
 * convention is far more stable across Zod wrapper kinds than the growing
 * set of `typeName`s.
 */
const ZOD_NESTED_SCHEMA_KEYS = [
  "type",
  "innerType",
  "schema",
  "left",
  "right",
  "valueType",
  "keyType",
  "options",
] as const;

/**
 * Duck-types a raw (pre-`toJsonSchema`) Zod value for `.refine(...)`/
 * `.transform(...)` (Zod's `ZodEffects` wrapper) and throws before the
 * caller's `toJsonSchema` adapter ever runs. `toJsonSchema` output is JSON
 * Schema, which has no way to represent "this value additionally requires a
 * runtime refinement/transform" — a refined/transformed field is otherwise
 * silently erased to its (accepted) inner scalar/object shape by the
 * adapter, defeating the `schema.zod` subset's rejection of semantic
 * widening.
 *
 * Walks the *entire* raw Zod definition graph reachable from `value` — not
 * just a whitelist of wrapper shapes `schema.zod` itself supports — so a
 * refinement/transform cannot hide behind a supported wrapper it merely
 * hasn't been explicitly special-cased for (e.g. `z.string().refine(...)
 * .brand()`: the outer `ZodBranded` wrapper is not itself an effect, but its
 * `_def.type` is). Cycle-safe via `seen` (`z.lazy()` can be self-referential)
 * so this does not need an actual Zod dependency — CDK doesn't depend on
 * Zod, so this reads Zod's `_def` shape structurally instead of importing
 * `zod`.
 */
function rejectZodEffects(value: unknown, adapterPath: string, seen: Set<object> = new Set()): void {
  if (typeof value !== "object" || value === null) return;
  if (seen.has(value)) return;
  seen.add(value);

  const def = (value as { readonly _def?: Record<string, unknown> })._def;
  if (typeof def !== "object" || def === null) return;

  if (def.typeName === "ZodEffects") {
    throw new Error(`${adapterPath}: refinements/transforms (z.refine/z.transform) are not supported`);
  }

  if (def.typeName === "ZodObject") {
    const rawShape = def.shape;
    const shape = typeof rawShape === "function" ? (rawShape as () => Record<string, unknown>)() : rawShape;
    if (typeof shape === "object" && shape !== null) {
      for (const field of Object.values(shape as Record<string, unknown>)) visitZodDefValue(field, adapterPath, seen);
    }
  }

  if (def.typeName === "ZodLazy" && typeof def.getter === "function") {
    visitZodDefValue((def.getter as () => unknown)(), adapterPath, seen);
  }

  for (const key of ZOD_NESTED_SCHEMA_KEYS) visitZodDefValue(def[key], adapterPath, seen);
}

function visitZodDefValue(raw: unknown, adapterPath: string, seen: Set<object>): void {
  if (Array.isArray(raw)) {
    for (const item of raw) visitZodDefValue(item, adapterPath, seen);
    return;
  }
  if (typeof raw === "object" && raw !== null) rejectZodEffects(raw, adapterPath, seen);
}

type JsonSchemaObject = Record<string, JsonValue>;

function schemaFromJsonSchema(id: string, source: unknown, adapterPath: string): SchemaHandle {
  validateSlug(id, "schema.id");
  const jsonSchema = normalizeJsonSchema(source, adapterPath);
  rejectUnsupportedKeywords(jsonSchema, adapterPath);
  const description = optionalString(jsonSchema.description, `${adapterPath}.description`);
  const type = jsonSchema.type;

  if (type === "object") {
    const properties = requiredObject(jsonSchema.properties, `${adapterPath}.properties`);
    const required = requiredNames(jsonSchema.required, `${adapterPath}.required`, properties);
    const fields: SchemaObjectFields = {};
    for (const [fieldId, fieldSchema] of Object.entries(properties)) {
      validateSlug(fieldId, `${adapterPath}.properties.${fieldId}`);
      fields[fieldId] = jsonSchemaField(fieldSchema, `${adapterPath}.properties.${fieldId}`, required.has(fieldId));
    }
    return schemaObject(id, fields, description === undefined ? {} : { description });
  }

  const schemaValue = jsonSchemaValue(jsonSchema, adapterPath);
  const allowed = enumValues(jsonSchema.enum, `${adapterPath}.enum`);
  if (allowed === undefined) throw new Error(`${adapterPath}: top-level schemas must be objects or scalar enums`);
  if (typeof schemaValue !== "string") throw new Error(`${adapterPath}: scalar enums cannot use array schemas`);
  assertEnumMatchesSchema(allowed, schemaValue, `${adapterPath}.enum`);
  return schemaEnumDeclaration(id, allowed, description === undefined ? {} : { description });
}

function jsonSchemaField(source: JsonValue, fieldPath: string, required: boolean): SchemaFieldFields {
  const jsonSchema = requiredJsonObject(source, fieldPath);
  rejectUnsupportedKeywords(jsonSchema, fieldPath);
  const description = optionalString(jsonSchema.description, `${fieldPath}.description`);
  const allowed = enumValues(jsonSchema.enum, `${fieldPath}.enum`);
  const schemaValue = jsonSchemaValue(jsonSchema, fieldPath);
  if (allowed !== undefined) assertEnumMatchesSchema(allowed, schemaValue, `${fieldPath}.enum`);
  return {
    schema: schemaValue,
    required,
    ...(description === undefined ? {} : { description }),
    ...(allowed === undefined ? {} : { allowed }),
  };
}

function jsonSchemaValue(jsonSchema: JsonSchemaObject, fieldPath: string): SchemaValue {
  if (jsonSchema.type === "string") return schema.text();
  if (jsonSchema.type === "number") return schema.number();
  if (jsonSchema.type === "integer") return schema.integer();
  if (jsonSchema.type === "boolean") return schema.boolean();
  if (jsonSchema.type === "array") {
    const items = requiredJsonObject(jsonSchema.items, `${fieldPath}.items`);
    rejectUnsupportedKeywords(items, `${fieldPath}.items`);
    if (items.enum !== undefined) throw new Error(`${fieldPath}.items.enum: array item enums are not supported`);
    return schemaList(jsonSchemaValue(items, `${fieldPath}.items`));
  }
  if (jsonSchema.type === "object") throw new Error(`${fieldPath}: nested objects are not supported`);
  throw new Error(`${fieldPath}.type: expected string, number, integer, boolean, array, or object`);
}

function normalizeJsonSchema(value: unknown, fieldPath: string): JsonSchemaObject {
  return requiredJsonObject(normalizeJsonValue(value, fieldPath, new Set<object>()), fieldPath);
}

function normalizeJsonValue(value: unknown, fieldPath: string, ancestors: Set<object>): JsonValue {
  if (typeof value === "number" && !Number.isFinite(value)) {
    throw new Error(`${fieldPath}: numbers must be finite`);
  }
  if (value === null || typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return value;
  }
  if (value === undefined || typeof value === "bigint" || typeof value === "function" || typeof value === "symbol") {
    throw new Error(`${fieldPath}: must contain JSON-only values`);
  }
  if (typeof value !== "object") throw new Error(`${fieldPath}: must contain JSON-only values`);
  if (ancestors.has(value)) throw new Error(`${fieldPath}: cyclic value is not JSON`);
  ancestors.add(value);
  const normalized: JsonValue = Array.isArray(value)
    ? value.map((item, index) => normalizeJsonValue(item, `${fieldPath}[${index}]`, ancestors))
    : stableObject(
        Object.fromEntries(
          Object.entries(value).map(([key, item]) => [key, normalizeJsonValue(item, `${fieldPath}.${key}`, ancestors)]),
        ),
      );
  ancestors.delete(value);
  return normalized;
}

function requiredJsonObject(value: JsonValue | unknown, fieldPath: string): JsonSchemaObject {
  if (value === null || Array.isArray(value) || typeof value !== "object") {
    throw new Error(`${fieldPath}: expected an object`);
  }
  return value as JsonSchemaObject;
}

function requiredObject(value: JsonValue | undefined, fieldPath: string): JsonSchemaObject {
  if (value === undefined) throw new Error(`${fieldPath}: is required for object schemas`);
  return requiredJsonObject(value, fieldPath);
}

function requiredNames(value: JsonValue | undefined, fieldPath: string, properties: JsonSchemaObject): Set<string> {
  if (value === undefined) return new Set();
  if (!Array.isArray(value) || !value.every((name) => typeof name === "string")) {
    throw new Error(`${fieldPath}: expected an array of strings`);
  }
  const required = new Set<string>();
  for (const name of value) {
    if (required.has(name)) throw new Error(`${fieldPath}: duplicate required property ${JSON.stringify(name)}`);
    if (!(name in properties)) throw new Error(`${fieldPath}: unknown required property ${JSON.stringify(name)}`);
    required.add(name);
  }
  return required;
}

function optionalString(value: JsonValue | undefined, fieldPath: string): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "string") throw new Error(`${fieldPath}: expected a string`);
  return value;
}

function enumValues(value: JsonValue | undefined, fieldPath: string): EnumLiteral[] | undefined {
  if (value === undefined) return undefined;
  if (
    !Array.isArray(value) ||
    !value.every((item) => typeof item === "string" || typeof item === "number" || typeof item === "boolean")
  ) {
    throw new Error(`${fieldPath}: expected an array of scalar values`);
  }
  return normalizeAllowedValues(value, fieldPath);
}

function rejectUnsupportedKeywords(schema: JsonSchemaObject, fieldPath: string): void {
  if (schema.additionalProperties !== undefined && schema.additionalProperties !== false) {
    throw new Error(`${fieldPath}.additionalProperties: unsupported JSON Schema keyword`);
  }
  const ignored = new Set(["type", "properties", "required", "items", "enum", "description", "additionalProperties"]);
  for (const key of Object.keys(schema)) {
    if (!ignored.has(key)) throw new Error(`${fieldPath}.${key}: unsupported JSON Schema keyword`);
  }
}
