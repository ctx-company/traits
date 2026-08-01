import type { JsonObject } from "./generated.js";
import type { InstructionOutputHandle, PortHandle, SlotHandle } from "./handles.js";
import type { CdkObject } from "./meta.js";
import { metaOf, withDeclaration } from "./meta.js";
import { collectMany, compact, mutableScalarArray, validateSlug } from "./normalize.js";
import { refText } from "./ref.js";
import type { SchemaValue } from "./schema.js";

export interface PortDefaultCommand {
  readonly cmd?: string;
  readonly argv?: readonly string[];
  readonly cwd?: string;
  readonly capture?: "stdout" | "stderr" | "exit-code" | readonly string[];
}

export interface PortFields {
  readonly id: string;
  readonly direction: "input" | "output";
  readonly schema: SchemaValue;
  readonly description?: string;
  /**
   * An `output.text`/`output.of(...)` instruction-output value must already
   * be attached to a step's `output:` (and thus resolved to a slot ref)
   * before this port declaration runs — author the producing step first.
   */
  readonly value?: SlotHandle | InstructionOutputHandle;
  readonly optional?: boolean;
  readonly title?: string;
  readonly format?: string | readonly string[];
  readonly default?: { readonly command?: PortDefaultCommand; } | { readonly cmd: string; };
}

/**
 * Port builder call signatures: declares a typed value at the BOUNDARY of a
 * trait or procedure — an input the caller supplies, or an output the
 * caller receives.
 *
 * Hand-writing a port as raw TOML/JSON means restating its schema ref, id,
 * and direction consistently everywhere it's referenced, with no static
 * check that a step reading the port even declares it as input. `port(...)`
 * returns a typed `PortHandle<Value>` instead of a hand-assembled blob: pass
 * it into `procedure({ input, output })` and into the sequence steps that
 * read or produce it, and passing the wrong *handle variable* (one bound to
 * a differently-typed port) at a call site expecting a specific `Value` is a
 * `tsc` error. That only holds when `Value` is inferred from an inline
 * built-in schema (`schema.text()`, `.number()`, ...); a port built from a
 * declared `schema.object`/`schema.enum` carries `Value = unknown`, and
 * whether the port's schema ref itself still resolves to a real declaration
 * is validated by the Rust synth/runtime, not `tsc`.
 *
 * **Ports vs slots**: a port is the trait/procedure's external contract —
 * what's visible to whoever calls the trait, or to another trait consuming
 * a shared procedure. A `slot` (see `slot.ts`) is procedure-LOCAL working
 * state — a place a step's output lands so a later step can read it — and
 * is never itself bindable across traits. An output port's `value` field is
 * commonly a slot: the port is the boundary declaration, the slot is where
 * the runtime actually holds the value between steps.
 *
 * @param fields Boundary declaration fields.
 * @example
 * ```ts
 * const diff = port.input.text({ id: "diff", description: "The diff to review." });
 * const review = slot.text("review");
 * const output = port.output.of({ id: "review", schema: ref.schema("code-review-scaffold"), value: review });
 * ```
 * @see {@link PortFields}
 * @see {@link slot}
 */
export interface PortFunction {
  <Value>(fields: PortFields & { readonly schema: SchemaValue<Value>; }): PortHandle<Value>;
  input: PortDirectionHelpers;
  output: PortDirectionHelpers;
}

/**
 * Direction-fixed port helpers reachable as `port.input.*`/`port.output.*`:
 * saves repeating `direction: "input"`/`"output"` (and its attendant risk of
 * a copy-pasted-wrong direction) at every port declaration.
 * @example `port.input.of(schema.number(), { id: "limit" })`
 */
export interface PortDirectionHelpers {
  /** Text-schema port in this fixed direction. @example `port.output.text({ id: "summary" })` */
  text(fields: Omit<PortFields, "direction" | "schema">): PortHandle<string>;
  /**
   * Port with an explicit schema in this fixed direction.
   * @example `port.input.of({ id: "limit", schema: schema.number() })`
   */
  of<Value>(fields: Omit<PortFields, "direction"> & { readonly schema: SchemaValue<Value>; }): PortHandle<Value>;
  /**
   * Port with the schema passed positionally instead of nested in `fields`.
   * @example `port.input.of(schema.number(), { id: "limit" })`
   */
  of<Value>(schemaValue: SchemaValue<Value>, fields: Omit<PortFields, "direction" | "schema">): PortHandle<Value>;
}

/**
 * Declares a typed trait boundary port.
 * @param fields Boundary direction, schema, and optional source slot.
 * @example `port.input.text({ id: "request" })`
 * @see {@link slot}
 */
function portFn<Value>(fields: PortFields & { readonly schema: SchemaValue<Value>; }): PortHandle<Value> {
  return portOf<Value>(fields);
}

// `Object.assign`'s own typing doesn't preserve a merged value's generic
// call signature precisely enough for structural comparison against
// `PortFunction`'s (each generic `Value` instantiation compares as a
// distinct type) — `portFn` and `portDirectionHelpers`'s `of` are each
// independently checked as real overloaded generics above; this is the one
// remaining cast bridging `Object.assign`'s merge, not an unchecked
// namespace-wide assertion.
export const port: PortFunction = Object.assign(
  portFn,
  { input: portDirectionHelpers("input"), output: portDirectionHelpers("output") },
) as PortFunction;

function portDirectionHelpers(direction: "input" | "output"): PortDirectionHelpers {
  function of<Value>(
    fields: Omit<PortFields, "direction"> & { readonly schema: SchemaValue<Value>; },
  ): PortHandle<Value>;
  function of<Value>(
    schemaValue: SchemaValue<Value>,
    fields: Omit<PortFields, "direction" | "schema">,
  ): PortHandle<Value>;
  function of(
    fieldsOrSchema: (Omit<PortFields, "direction"> & { readonly schema: SchemaValue; }) | SchemaValue,
    maybeFields?: Omit<PortFields, "direction" | "schema">,
  ): PortHandle {
    return typeof fieldsOrSchema === "string" || metaOf(fieldsOrSchema)?.ref !== undefined
      ? portOf({
        ...(maybeFields as Omit<PortFields, "direction" | "schema">),
        direction,
        schema: fieldsOrSchema as SchemaValue,
      })
      : portOf({ ...(fieldsOrSchema as Omit<PortFields, "direction">), direction });
  }
  return {
    text: (fields: Omit<PortFields, "direction" | "schema">): PortHandle<string> =>
      portOf<string>({ ...fields, direction, schema: "schema:text" }),
    of,
  };
}

// `Value` is forwarded straight into `withDeclaration`'s own `Value`
// parameter (P485 §3.4's single centralized mint in `withMeta`), rather than
// minted again here — `fields.schema` stays the untyped `PortFields["schema"]`
// deliberately: callers (`portFn`, `of`, `text`) each already know the
// concrete `Value` from their own generic parameter or a literal built-in
// schema ref, and supply it explicitly, since it isn't otherwise recoverable
// from a plain ref string like `"schema:text"`.
function portOf<Value = unknown>(fields: PortFields): PortHandle<Value> {
  validateSlug(fields.id, "port.id");
  const declaration = compact({
    id: fields.id,
    direction: fields.direction,
    schema: refText(fields.schema, "port.schema"),
    description: fields.description ?? `${fields.direction} port ${fields.id}.`,
    value: fields.value === undefined ? undefined : refText(fields.value, "port.value"),
    optional: fields.optional,
    title: fields.title,
    format: mutableScalarArray(fields.format),
    default: normalizePortDefault(fields.default),
  });
  const handle = withDeclaration<CdkObject, "port", Value>("port", `port:${fields.id}`, declaration, {}, {
    declarations: collectMany([fields.schema, fields.value]),
  });
  return handle;
}

function normalizePortDefault(value: PortFields["default"]): JsonObject | undefined {
  if (value === undefined) return undefined;
  return "cmd" in value ? { command: { cmd: value.cmd } } : value as JsonObject;
}
