import { recordTraitMint } from "./functional/context.js";
import type { JsonObject, JsonValue } from "./generated.js";
import type { InstructionOutputHandle, PortHandle, SlotHandle } from "./handles.js";
import type { CdkObject } from "./meta.js";
import { withDeclaration } from "./meta.js";
import { collectMany, compact, slugFromName } from "./normalize.js";
import { refText } from "./ref.js";
import type { SchemaValue } from "./schema.js";
import { schemaList } from "./schema.js";

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
  /** Advisory guidance about the value: examples or constraints for whoever
   * supplies it. Metadata only — the schema stays the validation contract.
   * The same field a slot carries, for the same reason. */
  readonly hint?: string;
  readonly default?:
    | { readonly value: string }
    | { readonly command?: PortDefaultCommand }
    | {
        readonly cmd: string;
      };
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
  /** Text port. @example `port.output.text("Summary")` */
  text(value: string | Omit<PortFields, "direction" | "schema">): PortHandle<string>;
  /** Boolean port — and a boolean INPUT port is a flag at the command line:
   * `ctx traits run work -- --draft` sets it true. @example `port.input.boolean("Draft")` */
  boolean(value: string | Omit<PortFields, "direction" | "schema">): PortHandle<boolean>;
  /** Numeric port. @example `port.input.number("Limit")` */
  number(value: string | Omit<PortFields, "direction" | "schema">): PortHandle<number>;
  /** List-of-text port. @example `port.output.texts("Findings")` */
  texts(value: string | Omit<PortFields, "direction" | "schema">): PortHandle<string[]>;
  /** List-of-number port. @example `port.output.numbers("Scores")` */
  numbers(value: string | Omit<PortFields, "direction" | "schema">): PortHandle<number[]>;
  /** Any JSON value, for shapes not worth declaring. @example `port.output.any("Raw")` */
  any(value: string | Omit<PortFields, "direction" | "schema">): PortHandle<JsonValue>;
  /** Curried list-port factory: bind the element schema once, declare several
   * ports of that list shape. @example `const findings = port.output.list(findingSchema); const report = findings("Report");` */
  list<Value>(
    schemaValue: SchemaValue<Value>,
  ): (value: string | Omit<PortFields, "direction" | "schema">) => PortHandle<Value[]>;
  /**
   * A port with an explicit schema. The barebones form — everything above is
   * this with the schema filled in.
   * @example `port.output.of("Report", schema.list(findingSchema))`
   * @example `port.output.of("Report", schema.list(findingSchema), { value: findings })`
   */
  of<Value>(
    name: string,
    schemaValue: SchemaValue<Value>,
    fields?: Omit<PortFields, "direction" | "schema" | "id">,
  ): PortHandle<Value>;
  /** Fields-only form, for a port assembled programmatically. */
  of<Value>(fields: Omit<PortFields, "direction"> & { readonly schema: SchemaValue<Value> }): PortHandle<Value>;
}

// `Object.assign`'s own typing doesn't preserve a merged value's generic
// call signature precisely enough for structural comparison against
// `PortFunction`'s (each generic `Value` instantiation compares as a
// distinct type) — `portFn` and `portDirectionHelpers`'s `of` are each
// independently checked as real overloaded generics above; this is the one
// remaining cast bridging `Object.assign`'s merge, not an unchecked
// namespace-wide assertion.
export const port: PortFunction = {
  input: portDirectionHelpers("input"),
  output: portDirectionHelpers("output"),
};

function portDirectionHelpers(direction: "input" | "output"): PortDirectionHelpers {
  // Every builtin is `of` with the schema filled in, so a port declared as
  // `port.input.text("Task")` and one declared as
  // `port.input.of("Task", schema.text())` cannot lower differently.
  const builtin =
    <Value>(schemaValue: SchemaValue<Value>) =>
    (value: string | Omit<PortFields, "direction" | "schema">): PortHandle<Value> =>
      portOf<Value>({
        ...(typeof value === "string" ? { id: value } : value),
        direction,
        schema: schemaValue,
      });

  function of<Value>(
    name: string,
    schemaValue: SchemaValue<Value>,
    fields?: Omit<PortFields, "direction" | "schema" | "id">,
  ): PortHandle<Value>;
  function of<Value>(
    fields: Omit<PortFields, "direction"> & { readonly schema: SchemaValue<Value> },
  ): PortHandle<Value>;
  function of(
    nameOrFields: string | (Omit<PortFields, "direction"> & { readonly schema: SchemaValue }),
    maybeSchema?: SchemaValue,
    maybeFields?: Omit<PortFields, "direction" | "schema" | "id">,
  ): PortHandle {
    if (typeof nameOrFields === "string") {
      return portOf({
        ...maybeFields,
        id: nameOrFields,
        direction,
        schema: maybeSchema as SchemaValue,
      });
    }
    return portOf({ ...nameOrFields, direction });
  }

  return {
    text: builtin<string>("schema:text"),
    boolean: builtin<boolean>("schema:boolean"),
    number: builtin<number>("schema:number"),
    texts: builtin<string[]>(schemaList<string>("schema:text")),
    numbers: builtin<number[]>(schemaList<number>("schema:number")),
    any: builtin<JsonValue>("schema:any"),
    list:
      <Value>(schemaValue: SchemaValue<Value>) =>
      (value: string | Omit<PortFields, "direction" | "schema">): PortHandle<Value[]> =>
        portOf<Value[]>({
          ...(typeof value === "string" ? { id: value } : value),
          direction,
          schema: schemaList(schemaValue),
        }),
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
  const id = slugFromName(fields.id, "port.id");
  const declaration = compact({
    id,
    direction: fields.direction,
    schema: refText(fields.schema, "port.schema"),
    description: fields.description ?? `${fields.direction} port ${id}.`,
    value: fields.value === undefined ? undefined : refText(fields.value, "port.value"),
    optional: fields.optional,
    title: fields.title,
    hint: fields.hint,
    default: normalizePortDefault(fields.default),
  });
  const handle = withDeclaration<CdkObject, "port", Value>(
    "port",
    `port:${id}`,
    declaration,
    {},
    {
      declarations: collectMany([fields.schema, fields.value]),
    },
  );
  recordTraitMint("port", id, `port:${id}`, declaration);
  return handle;
}

function normalizePortDefault(value: PortFields["default"]): JsonObject | undefined {
  if (value === undefined) return undefined;
  return "cmd" in value ? { command: { cmd: value.cmd } } : (value as JsonObject);
}
