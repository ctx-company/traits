import type { CanonicalGuardPredicate, RefKind } from "./generated.js";
import type { CdkObject } from "./meta.js";

declare const HANDLE_BRAND: unique symbol;
declare const HANDLE_VALUE: unique symbol;
declare const FIELD_REF_BRAND: unique symbol;

/** Phantom identity carried by every CDK handle. */
export type Brand<K extends string> = { readonly [HANDLE_BRAND]: K; };
type CdkHandle<K extends string, Value = unknown> = CdkObject & Brand<K> & {
  readonly [HANDLE_VALUE]?: Value;
};

/**
 * Attaches the same phantom `Value` marker `CdkHandle`/`Handle` carry, to an
 * already-branded object type — the one place `meta.ts`'s `withMeta`/
 * `withDeclaration` mint a handle's `Value` type parameter, so every builder
 * that returns e.g. `SchemaHandle<Value>`/`PortHandle<Value>`/`SlotHandle<
 * Value>` no longer has to re-assert the phantom itself at its own call site.
 */
export type WithHandleValue<T extends object, Value> = T & { readonly [HANDLE_VALUE]?: Value; };

/**
 * A typed reference to one field of an object-schema slot, produced by
 * `slot.foo` / `slot["exit-code"]` field access. Carries the parent slot ref
 * and field id privately (see `Meta.fieldRef`); `condition.equals(fieldRef,
 * value)` lowers it to the same canonical `{ slot, field, equals }` guard
 * `condition.fieldEquals` emits by hand.
 */
export type FieldRef<Value = unknown> = CdkObject & { readonly [FIELD_REF_BRAND]?: Value; };

/**
 * Maps an inferred object-schema value type to the field-ref proxy an
 * object-schema slot exposes: one `FieldRef<Value[K]>` per declared field.
 */
export type ObjectFieldRefs<Value> = { readonly [K in keyof Value]-?: FieldRef<Value[K]>; };

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
};
/**
 * A slot handle whose value is an object schema: exposes one `FieldRef`
 * per declared field (`slot.foo`, `slot["exit-code"]`) alongside the plain
 * `SlotHandle<Value>` surface. Scalar/list slots stay a bare `SlotHandle`.
 */
export type SlotWithFields<Value = unknown> =
  & SlotHandle<Value>
  & (
    Value extends readonly unknown[] ? unknown
      : Value extends object ? ObjectFieldRefs<Value>
      : unknown
  );
export type PortHandle<Value = unknown> = Handle<"port", Value>;
export type PromptHandle<Input = unknown, Output = unknown> = Handle<
  "prompt",
  { readonly input: Input; readonly output: Output; }
>;
export type ResourceHandle<Value = unknown> = Handle<"resource", Value>;
export type AgentHandle = Handle<"agent">;
export type SchemaHandle<Value = unknown> = Handle<"schema", Value>;
/** A built-in or collection schema reference with its represented value type. */
export type SchemaRef<Value = unknown> = string & { readonly __ctxTraitSchemaValue?: Value; };
export type SequenceHandle<Input = unknown, Output = unknown> = CdkHandle<
  "sequence-step",
  { readonly input: Input; readonly output: Output; }
>;
export type SequenceLinearHandle<Input = unknown, Output = unknown> = Handle<
  "sequence",
  { readonly input: Input; readonly output: Output; }
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
  { readonly input: Input; readonly output: Output; }
>;
export type TraitHandle = Handle<"trait">;
/**
 * The handle returned by `trait(id, { variants })`: a validated, flattened
 * variant family, not yet resolved for build. Not an `RefKind`-branded
 * `Handle` — a family is never referenced by ref the way a declared item is.
 */
export type TraitFamilyHandle = CdkHandle<"trait-family">;
export type PromptTemplate<Input = unknown> = CdkHandle<"prompt-template", Input>;
export type OutputSinkHandle<Value = unknown> = SlotHandle<Value> | CdkHandle<"output-sink", Value>;
/**
 * An `output.text`/`output.of(schema)` instruction-output: carries the
 * compiled instruction text (and, for `.of`, the schema ref) until the step
 * that lists it in `output:` auto-declares its backing slot and resolves
 * this handle's ref to it. Accepted anywhere a `SlotHandle` is (a sequence
 * step's `output:`, and — once attached — a later step's interpolated
 * `input.text`/`prompt.text`).
 */
export type InstructionOutputHandle<Value = unknown> = CdkHandle<"instruction-output", Value>;
/** A typed reference to a declared `[[session]]`, returned by `session(id, opts)`. */
export type SessionHandle = Handle<"session">;
/** A typed reference to a declared `[[signal]]`, returned by `signal(fields)`. */
export type SignalHandle = Handle<"signal">;
