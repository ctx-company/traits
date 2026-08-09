/**
 * Shared internal helpers for the functional-authoring layer
 * (`context.ts`, `trait.ts`, `procedure.ts`). Not part of the public API.
 */

/** Structural check for a `PromiseLike`, used to detect async trait/step bodies without importing a Promise polyfill type. */
export function isThenable(value: unknown): value is PromiseLike<unknown> {
  return typeof value === "object" && value !== null && typeof (value as { then?: unknown; }).then === "function";
}
