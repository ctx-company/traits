import type { SettingHandle as ConfigSettingHandle } from "@ctx-traits/config";
import type { EachParam } from "./functional/registrars.js";
import type { CanonicalGuardPredicate, JsonValue, RefKind, WriteOperation } from "./generated.js";
import type { CdkObject } from "./meta.js";
import type { PromptRegistrarOptions } from "./sequence.js";

declare const HANDLE_BRAND: unique symbol;
declare const HANDLE_VALUE: unique symbol;
declare const FIELD_REF_BRAND: unique symbol;

/** Phantom identity carried by every CDK handle. */
export type Brand<K extends string> = { readonly [HANDLE_BRAND]: K };
type CdkHandle<K extends string, Value = unknown> = CdkObject &
  Brand<K> & {
    readonly [HANDLE_VALUE]?: Value;
  };

/**
 * Attaches the same phantom `Value` marker `CdkHandle`/`Handle` carry, to an
 * already-branded object type — the one place `meta.ts`'s `withMeta`/
 * `withDeclaration` mint a handle's `Value` type parameter, so every builder
 * that returns e.g. `SchemaHandle<Value>`/`PortHandle<Value>`/`SlotHandle<
 * Value>` no longer has to re-assert the phantom itself at its own call site.
 */
export type WithHandleValue<T extends object, Value> = T & { readonly [HANDLE_VALUE]?: Value };

/**
 * A typed reference to one field of an object-schema slot, produced by
 * `slot.foo` / `slot["exit-code"]` field access. Carries the parent slot ref
 * and field id privately (see `Meta.fieldRef`); `condition.equals(fieldRef,
 * value)` lowers it to the same canonical `{ slot, field, equals }` guard
 * `condition.fieldEquals` emits by hand.
 */
export type FieldRef<Value = unknown> = CdkObject & { readonly [FIELD_REF_BRAND]?: Value };

/**
 * Maps an inferred object-schema value type to the field-ref proxy an
 * object-schema slot exposes: one `FieldRef<Value[K]>` per declared field.
 * When a field's own value is itself an object (not a list), the field ref
 * recurses and re-exposes `ObjectFieldRefs<Value[K]>` too, so `slot.a.b.c`
 * typechecks — mirroring `slot.a` minting a dotted `fieldRef.field` instead
 * of a plain name. Lists stop the recursion: a path is a sequence of object
 * field names only (task 0085's decision), no list indexing/wildcards.
 */
export type ObjectFieldRefs<Value> = {
  readonly [K in keyof Value]-?: FieldRef<Value[K]> &
    // `unknown` here is the intersection identity (`T & unknown` = `T`) — a list or scalar field
    // contributes no extra field-ref surface beyond `FieldRef<Value[K]>` above.
    (Value[K] extends readonly unknown[] ? unknown : Value[K] extends object ? ObjectFieldRefs<Value[K]> : unknown);
};

/** A typed opaque reference emitted by a CDK builder. */
export type Handle<K extends RefKind, Value = unknown> = CdkHandle<K, Value>;
export type RefHandle<K extends RefKind = RefKind, Value = unknown> = Handle<K, Value>;

/**
 * A per-site optionality wrapper for a slot read, identical in canonical
 * output to `input.optional(slot)` — `{ slot, optional: true }`. Optionality
 * has never been a property of the slot: the same slot stays required at one
 * step and optional at another. Reading a slot optionally is what makes a
 * loop-carried read expressible — INCLUDING a step reading the slot it also
 * writes, because the producer-before-consumer check exempts optional inputs
 * (`modules/core/src/trait/procedure/validate.rs`), "read the current value
 * if one exists" being exactly loop semantics.
 */
export type OptionalSlotRead<Value = unknown> = {
  readonly slot: SlotHandle<Value>;
  readonly optional: true;
};

export type SlotHandle<Value = unknown> = Handle<"slot", Value>;

/**
 * A slot handle from a declaration builder, which additionally exposes
 * `.optional()`. Kept distinct from the bare {@link SlotHandle} so a plain
 * `ref.slot("x")` reference — which declares nothing and carries no
 * augmentation — still satisfies every `SlotHandle` position.
 */
export type DeclaredSlotHandle<Value = unknown> = SlotHandle<Value> & {
  readonly optional: () => OptionalSlotRead<Value>;
  /** `.forEach` is attached non-enumerably at mint time (0106, `slot.ts`) — `items.forEach` is THE for-each spelling (0102). The body's second parameter, `each`, is where every knob (`limit`/`maxItems`/`concurrent`/`itemSchema`) lives (0211). */
  readonly forEach: (title: string, body: (item: SlotHandle, each: EachParam) => void) => SequenceHandle;
  /** `.with` is attached non-enumerably at mint time (0210, `slot.ts`) — the authoring form of `operation.over(slot, op)`. */
  readonly with: SlotSink<Value>;
};
/**
 * A declared object-schema slot handle: `DeclaredSlotHandle`'s `.optional()`
 * plus `SlotWithFields`'s per-field `FieldRef` access — what `slot(...)`/
 * `slot.of(...)` actually return for an object schema.
 */
export type DeclaredSlotWithFields<Value = unknown> = DeclaredSlotHandle<Value> & SlotWithFields<Value>;
/**
 * A slot handle whose value is an object schema: exposes one `FieldRef`
 * per declared field (`slot.foo`, `slot["exit-code"]`) alongside the plain
 * `SlotHandle<Value>` surface. Scalar/list slots stay a bare `SlotHandle`.
 */
export type SlotWithFields<Value = unknown> = SlotHandle<Value> &
  // `unknown` here is the intersection identity — a list or scalar value adds no field-ref surface.
  (Value extends readonly unknown[] ? unknown : Value extends object ? ObjectFieldRefs<Value> : unknown);
/**
 * The check-step verdict record (P565): a check's output slot declares at
 * least `ok` (schema:boolean) and `argv` (a list of schema:text) — the argv
 * makes the gate self-describing, so a bare `false` can never send a
 * consumer re-validating with whatever command the surrounding prose names.
 * Extra declared fields are fine; the runtime validates the minimum. Lives
 * here (not sequence.ts) so condition.ts can name it without a module
 * cycle.
 */
export type CheckResultValue = { readonly ok: boolean; readonly argv: readonly string[] };
export type PortHandle<Value = unknown> = Handle<"port", Value>;
/**
 * A typed operator knob: declared via `@ctx-traits/config`'s `setting.*`
 * (0180 moved the declaration API there), resolved from config layers at
 * activation. Accepted anywhere a `SlotHandle`/`PortHandle` is — prompt
 * interpolation, argv tokens, condition operands, structural fields (e.g.
 * the loop bound) — type-checked the same way. Re-exported (not rebuilt on
 * `Handle`/`Brand`): a config-minted handle carries no CDK brand.
 */
export type SettingHandle<Value = unknown> = ConfigSettingHandle<Value>;
export type PromptHandle<Input = unknown, Output = unknown> = Handle<
  "prompt",
  { readonly input: Input; readonly output: Output }
>;
export type ResourceHandle<Value = unknown> = Handle<"resource", Value>;
/**
 * An agent handle from a declaration builder — `agent(...)` and every
 * `agent.*` template — exposing `.prompt`, attached non-enumerably at mint
 * time (0106, `agent.ts`). One type, not a declared/bare-ref split: the
 * un-augmented `ref.agent` form was dropped with zero call sites (owner
 * ruling 2026-08-11), so every agent handle in circulation comes from a
 * builder and carries the registrar.
 */
export type AgentHandle = Handle<"agent"> & {
  readonly prompt: (title: string, opts: PromptRegistrarOptions) => SequenceHandle;
};
export type SchemaHandle<Value = unknown> = Handle<"schema", Value>;
/** A built-in or collection schema reference with its represented value type. */
export type SchemaRef<Value = unknown> = string & { readonly __ctxTraitSchemaValue?: Value };
export type SequenceHandle<Input = unknown, Output = unknown> = CdkHandle<
  "sequence-step",
  { readonly input: Input; readonly output: Output }
>;
export type SequenceLinearHandle<Input = unknown, Output = unknown> = Handle<
  "sequence",
  { readonly input: Input; readonly output: Output }
>;
export type ConditionHandle = Handle<"condition">;
/**
 * A single guard predicate returned by every `condition.*` builder — the
 * closed canonical `CanonicalGuardPredicate` shape plus the phantom `"guard"`
 * brand, deliberately not built on `CdkHandle`/`CdkObject`: `CdkObject`'s
 * index signature is exactly the hole a typed guard closes.
 */
export type GuardHandle = CanonicalGuardPredicate & Brand<"guard">;
export type RuleHandle = Handle<"rule">;
export type ProcedureHandle<Input = unknown, Output = unknown> = CdkHandle<
  "procedure",
  { readonly input: Input; readonly output: Output }
>;
export type TraitHandle = Handle<"trait">;
/**
 * The handle returned by `trait(id, { variants })`: a validated, flattened
 * variant family, not yet resolved for build. Not an `RefKind`-branded
 * `Handle` — a family is never referenced by ref the way a declared item is.
 */
export type TraitFamilyHandle = CdkHandle<"trait-family">;
/**
 * Values that can be interpolated into a prompt and therefore become prompt
 * inputs. Defined HERE, not in prompt.ts: PromptTemplate.extend below
 * references this union, and hosting it in a module that imports handles
 * created the cross-file type-alias cycle TS2456 rejects under
 * bundler-resolution consumers (trait-source tsc, tsserver). prompt.ts
 * re-exports it, so the public import path is unchanged.
 */
export type PromptInterpolation<Value = unknown> =
  | PortHandle<Value>
  | SlotHandle<Value>
  | SettingHandle<Value>
  | ResourceHandle<Value>
  | InstructionOutputHandle<Value>
  | OptionalSlotRead<Value>
  | PromptTemplate<Value>;

/** One `output.prompt` interpolation site: a slot or an optional slot read. Lives here for the same anti-cycle reason; output.ts re-exports. */
export type OutputPromptInterpolation<Value = unknown> = SlotHandle<Value> | OptionalSlotRead<Value>;

/**
 * `.extend`'s callable shape — an INTERFACE so the mutual
 * PromptInterpolation/PromptTemplate reference resolves lazily instead of
 * tripping TS2456 alias-cycle detection in consumers that check the dist
 * declarations (trait-source tsc without skipLibCheck, tsserver).
 */
export interface PromptTemplateExtend<Input = unknown> {
  (strings: TemplateStringsArray, ...values: readonly PromptInterpolation[]): PromptTemplate<Input>;
}

export type PromptTemplate<Input = unknown> = CdkHandle<"prompt-template", Input> & {
  /**
   * Returns a NEW template with the extension's text appended (newline
   * join); the receiver is unchanged, so a shared base extended by two
   * variants never accumulates. Chainable: `p.extend`a`.extend`b``.
   */
  readonly extend: PromptTemplateExtend<Input>;
};
export type OutputSinkHandle<Value = unknown> = SlotHandle<Value> | CdkHandle<"output-sink", Value>;
/**
 * The call signature of a slot handle's `.with` method (0210, 0207 ruling
 * 4) — the authoring-form spelling of `operation.over(slot, op)`, typed per
 * write mode exactly like `operation.over`'s own overloads: append needs a
 * list-valued slot, increment a number-valued slot, `merge`/`set-field` and
 * no-arg/`replace` are available on every slot.
 */
export type SlotSink<Value> = ((
  operation: "merge" | Extract<WriteOperation, { readonly "set-field": string }>,
) => OutputSinkHandle<JsonValue>) &
  ((operation?: "replace") => OutputSinkHandle<Value>) &
  // `unknown` here is the intersection identity (`T & unknown` = `T`) — a non-list/non-number
  // value contributes no append/increment call signature beyond the two above.
  (Value extends readonly (infer Element)[] ? (operation: "append") => OutputSinkHandle<Element> : unknown) &
  (Value extends number ? (operation: "increment") => OutputSinkHandle<number> : unknown);
/**
 * An `output.text`/`output.of(schema)` instruction-output: carries the
 * compiled instruction text (and, for `.of`, the schema ref) until the step
 * that lists it in `output:` auto-declares its backing slot and resolves
 * this handle's ref to it. Accepted anywhere a `SlotHandle` is (a sequence
 * step's `output:`, and — once attached — a later step's interpolated
 * `input.prompt`).
 */
export type InstructionOutputHandle<Value = unknown> = CdkHandle<"instruction-output", Value>;
/**
 * An `output.prompt` tagged-template value: the prose is the attaching
 * step's instruction (appended to its compiled prompt), and each
 * interpolated slot (optionally `.optional()`) IS the step's output
 * contract — instructions and contract cannot drift apart, the output-side
 * mirror of `input.prompt`. Accepted anywhere a step's `output:` accepts a
 * slot handle; expands into ordinary output-sink entries at lowering.
 */
export type OutputTemplateHandle = CdkHandle<"output-template"> & {
  /** Same composition contract as `PromptTemplate.extend`, merging the output contract (`refs`/`optionalRefs`/`slots`). */
  readonly extend: (
    strings: TemplateStringsArray,
    ...values: readonly OutputPromptInterpolation[]
  ) => OutputTemplateHandle;
};
/** A typed reference to a declared `[[session]]`, returned by `session(id, opts)`. */
export type SessionHandle = Handle<"session">;
/** A typed reference to a declared `[[signal]]`, returned by `signal(fields)`. */
export type SignalHandle = Handle<"signal">;
