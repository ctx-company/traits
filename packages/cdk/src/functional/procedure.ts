/**
 * The `procedure.from(fields, body)` adapter (0106): opens a build, runs
 * `body` synchronously, and feeds the collected root-scope items into the
 * existing `procedure(...)` builder — so duplicate-title checks,
 * normalization, diagnostics, and source maps all run on the object layer's
 * existing path, and the returned `ProcedureHandle` drops into
 * `variant({ procedure })` unchanged.
 */
import type { ProcedureHandle, SequenceHandle } from "../handles.js";
import type { ProcedureFields } from "../procedure.js";
import { closeBuild, openBuild } from "./context.js";

export type ProcedureFromFields = Omit<ProcedureFields, "sequence">;

export interface ProcedureFromFunction {
  /** Authors a procedure functionally: `body` registers steps via `step.*`/`agent.prompt`/`flow.*`, in the order they should run. */
  from(fields: ProcedureFromFields, body: () => void): ProcedureHandle;
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return typeof value === "object" && value !== null && typeof (value as { then?: unknown; }).then === "function";
}

/** Attaches `.from` onto the existing object-layer `procedure` function, so `procedure.from(...)` and `procedure(...)` share one exported symbol — no separate colliding export. */
export function attachProcedureFrom<T extends (fields: ProcedureFields) => ProcedureHandle>(
  base: T,
): T & ProcedureFromFunction {
  return Object.assign(base, {
    from: (fields: ProcedureFromFields, body: () => void): ProcedureHandle => {
      openBuild();
      let result: unknown;
      let items: readonly { readonly item: unknown; }[];
      try {
        result = body();
      } finally {
        items = closeBuild();
      }
      if (isThenable(result)) {
        throw new Error("procedure.from: async callbacks are not supported — the body must run synchronously");
      }
      const sequenceItems = items.map((registered) => registered.item as SequenceHandle);
      return base({ ...fields, sequence: sequenceItems });
    },
  });
}
