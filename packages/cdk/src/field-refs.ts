import type { JsonObject } from "./generated.js";
import type { Meta } from "./meta.js";
import { attachMeta, metaOf } from "./meta.js";
import { collectMany } from "./normalize.js";
import type { SchemaValue } from "./schema.js";

/** One declared object-schema field's canonical shape, as it appears in `declaration.fields`. */
export interface SchemaFieldSummary {
  readonly schema: string;
}

/** Narrows a JSON field map to `declaration.fields`' actual per-field shape without a cast — every value must itself declare a `schema` ref. */
function isSchemaFieldSummaryRecord(fields: JsonObject): fields is JsonObject & Record<string, SchemaFieldSummary> {
  return Object.values(fields).every(
    (value) =>
      typeof value === "object" &&
      value !== null &&
      !Array.isArray(value) &&
      typeof (value as JsonObject).schema === "string",
  );
}

/**
 * The declared fields of an object schema (`schema.object`/`extend`/
 * `template`), keyed by field id, or `undefined` when `schemaValue` isn't a
 * declared object schema (a built-in, list, union, or enum schema has no
 * per-field access).
 */
export function objectSchemaFields(schemaValue: SchemaValue): Record<string, SchemaFieldSummary> | undefined {
  const fields = (metaOf(schemaValue)?.declaration as { readonly fields?: JsonObject } | undefined)?.fields;
  return fields === undefined ||
    fields === null ||
    typeof fields !== "object" ||
    Array.isArray(fields) ||
    !isSchemaFieldSummaryRecord(fields)
    ? undefined
    : fields;
}

/**
 * The declared fields of the local object schema `ref` points at, looked up
 * in an already-collected declaration set — or `undefined` when `ref` isn't
 * a local schema ref, isn't declared, or declares no inline fields (a
 * resource-backed or scalar-enum schema has no per-field access, and a
 * field pointing at one is a recursion leaf).
 */
export function objectSchemaFieldsByRef(
  ref: string,
  declarations: ReturnType<typeof collectMany>,
): Record<string, SchemaFieldSummary> | undefined {
  if (!ref.startsWith("schema:") || ref.includes(":", "schema:".length)) return undefined;
  const id = ref.slice("schema:".length);
  const declaration = declarations.schema?.find((candidate) => (candidate as { readonly id?: string }).id === id) as
    | { readonly fields?: JsonObject }
    | undefined;
  const fields = declaration?.fields;
  return fields === undefined ||
    fields === null ||
    typeof fields !== "object" ||
    Array.isArray(fields) ||
    !isSchemaFieldSummaryRecord(fields)
    ? undefined
    : fields;
}

/**
 * Wraps an object-schema handle (a slot or, in future, a signal) — or,
 * recursively, an object-schema `FieldRef` — in a proxy exposing one
 * `FieldRef` per declared field (`slot.foo`, `slot["exit-code"]`) alongside
 * the base value's normal surface. When an accessed field's own declared
 * schema is itself a local object schema, the returned `FieldRef` recurses
 * the same way, so `slot.a.b.c` mints a field ref for `a.b.c` — task 0085's
 * dot-joined path encoding. Lists and scalar leaves stop the recursion.
 *
 * Uses own-field lookup — so a name like `constructor` cannot resolve
 * through `Object.prototype` — and throws, naming the owner and the full
 * dotted path, on any other unknown string access; symbol access (CDK
 * metadata, `Symbol.toPrimitive`, ...) always passes through untouched.
 *
 * Every minted field ref, at any depth, carries the same parent
 * `declarations` (the owner's declaration plus every transitively-collected
 * schema declaration reachable from it), so a guard built off a field ref
 * collects exactly what a hand-written guard would — omitting this would let
 * a trait authored through field access silently drop the owner/schema
 * declarations from the built trait.
 *
 * @param ownerKind Owner-kind label used in the unknown-field error message
 * (`"slot"`, `"signal"`).
 * @param mintFieldMeta Builds the `Meta` a leaf field ref carries — slots
 * mint `{ fieldRef: { slotRef, field } }`, signals mint `{ ref:
 * "signal:<id>.<field>" }`; each owner mints only the meta shape its own
 * consumers (condition.equals, prompt interpolation) need.
 */
export function fieldRefProxy(
  base: object,
  ownerKind: string,
  ownerId: string,
  fieldPath: readonly string[],
  fields: Record<string, SchemaFieldSummary>,
  declarations: ReturnType<typeof collectMany>,
  mintFieldMeta: (nextPath: readonly string[], declarations: ReturnType<typeof collectMany>) => Meta,
): object {
  const fieldRefCache = new Map<string, object>();
  return new Proxy(base, {
    get(target, prop, receiver) {
      if (typeof prop === "symbol" || Object.hasOwn(target, prop)) return Reflect.get(target, prop, receiver);
      const field = Object.hasOwn(fields, prop) ? fields[prop] : undefined;
      if (field === undefined) {
        const fullPath = [...fieldPath, prop].join(".");
        throw new Error(`${ownerKind} ${JSON.stringify(ownerId)} has no field ${JSON.stringify(fullPath)}`);
      }
      let fieldRef = fieldRefCache.get(prop);
      if (fieldRef === undefined) {
        const nextPath = [...fieldPath, prop];
        const leaf = attachMeta({}, mintFieldMeta(nextPath, declarations));
        const nestedFields = objectSchemaFieldsByRef(field.schema, declarations);
        fieldRef =
          nestedFields === undefined
            ? leaf
            : fieldRefProxy(leaf, ownerKind, ownerId, nextPath, nestedFields, declarations, mintFieldMeta);
        fieldRefCache.set(prop, fieldRef);
      }
      return fieldRef;
    },
  });
}
