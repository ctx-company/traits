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
import type { SessionTitleSinkInput } from "../sink.js";
import { attachSessionTitleSink } from "../sink.js";
import { closeBuild, openBuild } from "./context.js";
import { isThenable } from "./internal.js";

export type ProcedureFromFields = Omit<ProcedureFields, "sequence">;

export interface ProcedureFromFunction {
  /** Authors a procedure functionally: `body` registers steps via `step.*`/`agent.prompt`/`flow.*`, in the order they should run. */
  from(fields: ProcedureFromFields, body: () => void): ProcedureHandle;
}

/** Attaches `.from` onto the existing object-layer `procedure` function, so `procedure.from(...)` and `procedure(...)` share one exported symbol — no separate colliding export. */
export function attachProcedureFrom<T extends (fields: ProcedureFields) => ProcedureHandle>(
  base: T,
): T & ProcedureFromFunction {
  return Object.assign(base, {
    from: (fields: ProcedureFromFields, body: () => void): ProcedureHandle => {
      openBuild();
      let result: unknown;
      let closed: { readonly items: readonly { readonly item: unknown; }[]; readonly sessionTitleSink: unknown; };
      try {
        result = body();
      } finally {
        closed = closeBuild();
      }
      if (isThenable(result)) {
        throw new Error("procedure.from: async callbacks are not supported — the body must run synchronously");
      }
      const sequenceItems = closed.items.map((registered) => registered.item as SequenceHandle);
      const handle = base({ ...fields, sequence: sequenceItems });
      return closed.sessionTitleSink === undefined
        ? handle
        : attachSessionTitleSink(handle, closed.sessionTitleSink as SessionTitleSinkInput);
    },
  });
}
