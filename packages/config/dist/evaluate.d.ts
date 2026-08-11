import type { CtxConfig } from "./generated.js";
export interface EvaluateConfigFunctionOptions {
    /** The layer the build is targeting — `defineRepo` is only legal under `"user-global"`. */
    layer?: "user-global" | "repo";
}
/**
 * Evaluates a hook-style config default export — a function of `define*`
 * calls — into a `CtxConfig` document. Opens the ambient config frame,
 * calls `fn()` synchronously (an async default export is a named error,
 * same rule the CDK enforces for trait functions), closes the frame in
 * `finally`, then assembles the collected tables. Only called from the
 * `ctx traits config build` emit script — nothing on any load path calls
 * this.
 *
 * A `defineConfig({...})` call reached while this frame is open is the
 * mixed-form error — `defineConfig` (see `index.ts`) throws it directly, so
 * this function never needs to detect it after the fact.
 */
export declare function evaluateConfigFunction(fn: () => unknown, opts?: EvaluateConfigFunctionOptions): CtxConfig;
//# sourceMappingURL=evaluate.d.ts.map