import type { RefKind } from "./generated.js";
import type {
  ConditionHandle,
  Handle,
  PortHandle,
  PromptHandle,
  RefHandle,
  ResourceHandle,
  RuleHandle,
  SchemaHandle,
  SequenceLinearHandle,
  SlotHandle,
  TraitHandle,
} from "./handles.js";
import type { CdkObject } from "./meta.js";
import { metaOf, withMeta } from "./meta.js";
import { validateSlug } from "./normalize.js";

/** Canonical string form of a reference, including optional dependency paths. */
export type RefText<K extends RefKind = RefKind, Id extends string = string> = `${K}:${Id}`;

export interface RefFunction {
  /** Bare `"kind:id"` ref string, untyped — prefer a typed `ref.*` kind below when one exists. @example `ref("resource", "style-guide")` */
  <K extends RefKind, Id extends string>(kind: K, id: Id): RefText<K, Id>;
  /** Typed reference to a port declared elsewhere. @example `ref.port({ id: "diff", dependency: "shared" })` */
  port<Value = unknown>(id: string | { readonly id: string; readonly dependency?: string }): PortHandle<Value>;
  /** Typed reference to a slot declared elsewhere. @example `ref.slot("review")` */
  slot<Value = unknown>(id: string | { readonly id: string; readonly dependency?: string }): SlotHandle<Value>;
  /** Typed reference to a named prompt declared elsewhere. @example `ref.prompt("instructions")` */
  prompt<Input = unknown, Output = unknown>(
    id: string | { readonly id: string; readonly dependency?: string },
  ): PromptHandle<Input, Output>;
  /** Typed reference to a declared schema. @example `port.output.of({ id: "review", schema: ref.schema("code-review-scaffold"), value: review })` */
  schema<Value = unknown>(id: string | { readonly id: string; readonly dependency?: string }): SchemaHandle<Value>;
  /** Typed reference to a named `sequence.linear` declaration. @example `ref.sequence("refine-work")` */
  sequence<Input = unknown, Output = unknown>(
    id: string | { readonly id: string; readonly dependency?: string },
  ): SequenceLinearHandle<Input, Output>;
  /** Typed reference to a named condition declaration. @example `ref.condition("both-approved")` */
  condition(id: string | { readonly id: string; readonly dependency?: string }): ConditionHandle;
  /** Typed reference to a declared resource. @example `prompt.resource(ref.resource("style-guide"))` */
  resource<Value = unknown>(id: string | { readonly id: string; readonly dependency?: string }): ResourceHandle<Value>;
  /** Typed reference to a declared `[rule.*]` entry. @example `ref.rule("safe")` */
  rule(id: string | { readonly id: string; readonly dependency?: string }): RuleHandle;
  /** Typed reference to a declared signal. @example `condition.signal(ref.signal("approved"))` */
  signal(id: string | { readonly id: string; readonly dependency?: string }): RefHandle<"signal">;
  /** Typed reference to a declared `dependency(...)` trait. @example `ref.trait("shared")` */
  trait(id: string | { readonly id: string; readonly dependency?: string }): TraitHandle;
}

/**
 * Creates typed references without declaring the referenced item: a handle
 * that points at a port/slot/prompt/schema/... declared elsewhere (often in
 * a dependency trait), for when you need to name it but this file isn't
 * where it's built.
 *
 * A bare ref string like `"schema:code-review-scaffold"` compiles fine but
 * carries no type information and no check that the target actually exists
 * — a typo is caught only at synth time. `ref.schema(id)` (and the other
 * `ref.*` kinds) returns a handle typed for the ref *kind* (`SchemaHandle`,
 * `PortHandle`, ...) so it's accepted only where the CDK expects that kind
 * of handle, and `id` is validated as a well-formed slug when the CDK
 * source runs (build-script time, not `tsc`). Neither of those checks
 * proves the target actually exists: `ref.schema("typo-of-a-real-id")`
 * type-checks and passes slug validation identically to a correct id — a
 * renamed or missing declaration is only caught later, by the Rust
 * synth/runtime that resolves every ref against the traits it actually
 * declares.
 *
 * Pass `{ id, dependency }` instead of a bare id to reach into a declared
 * `dependency(...)` trait rather than this trait's own declarations.
 *
 * @param kind Reference kind.
 * @example
 * ```ts
 * const review = slot.text("review");
 * const output = port.output.of({ id: "review", schema: ref.schema("code-review-scaffold"), value: review });
 * ```
 * @see {@link refText}
 */
type RefIdArg = string | { readonly id: string; readonly dependency?: string };

// `.bind(undefined, kind)` erases `refHandle`'s own `Value` generic to a
// fixed `Handle<K>` (`Value = unknown`) per bound function — the interface's
// `ref.port<Value = unknown>(id): PortHandle<Value>`-style members are each
// independently generic, which a `.bind` result can't reconstruct. Each
// per-kind wrapper below is a small real generic function instead, forwarding
// its own `Value` into `refHandle`'s own `Value` parameter (P485 §3.4's
// single centralized mint in `withMeta`) rather than re-asserting the
// phantom locally — so only the whole-namespace `Object.assign` merge (same
// limitation documented in `port.ts`/`slot.ts`) needs the outer cast.
function refPort<Value = unknown>(id: RefIdArg): PortHandle<Value> {
  return refHandle<"port", Value>("port", id);
}
function refSlot<Value = unknown>(id: RefIdArg): SlotHandle<Value> {
  return refHandle<"slot", Value>("slot", id);
}
function refPrompt<Input = unknown, Output = unknown>(id: RefIdArg): PromptHandle<Input, Output> {
  return refHandle<"prompt", { readonly input: Input; readonly output: Output }>("prompt", id);
}
function refSchema<Value = unknown>(id: RefIdArg): SchemaHandle<Value> {
  return refHandle<"schema", Value>("schema", id);
}
function refSequence<Input = unknown, Output = unknown>(id: RefIdArg): SequenceLinearHandle<Input, Output> {
  return refHandle<"sequence", { readonly input: Input; readonly output: Output }>("sequence", id);
}
function refResource<Value = unknown>(id: RefIdArg): ResourceHandle<Value> {
  return refHandle<"resource", Value>("resource", id);
}

export const ref = Object.assign((kind: RefKind, id: string): string => `${kind}:${id}`, {
  port: refPort,
  slot: refSlot,
  prompt: refPrompt,
  schema: refSchema,
  sequence: refSequence,
  condition: (id: RefIdArg): ConditionHandle => refHandle("condition", id),
  resource: refResource,
  rule: (id: RefIdArg): RuleHandle => refHandle("rule", id),
  signal: (id: RefIdArg): RefHandle<"signal"> => refHandle("signal", id),
  trait: (id: RefIdArg): TraitHandle => refHandle("trait", id),
}) as RefFunction;

function refHandle<K extends RefKind, Value = unknown>(kind: K, value: RefIdArg): Handle<K, Value> {
  const id = typeof value === "string" ? value : value.id;
  const dependency = typeof value === "string" ? undefined : value.dependency;
  validateSlug(id, `${kind}.id`);
  const path = dependency === undefined ? id : `${dependency}/${id}`;
  return withMeta<CdkObject, "ref", Value, K>({ ref: `${kind}:${path}` }, { kind: "ref", ref: `${kind}:${path}` });
}

export function refText(value: unknown, fieldPath: string): string {
  const ref = metaOf(value)?.ref;
  if (ref !== undefined) {
    return ref;
  }
  if (typeof value === "string") {
    return value;
  }
  throw new Error(`${fieldPath}: expected typed ref handle or ref string`);
}
