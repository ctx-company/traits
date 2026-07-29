import type { SessionHandle } from "./handles.js";
import { withDeclaration } from "./meta.js";
import { compact, refText, validateSlug } from "./normalize.js";

export interface SessionOptions {
  readonly description?: string;
}

/** The two no-sharing session lifecycle constants: agent-local, never shared. */
export type SessionLifecycle = "per-frame" | "persistent";

/**
 * What an `agent(...)`/role-template `session` field accepts: a named,
 * shareable `session(...)` handle, or a bare lifecycle constant
 * (`session.PerFrame`/`session.Persistent`) that never creates a shared
 * declaration.
 */
export type SessionBinding = SessionHandle | SessionLifecycle;

export interface SessionFunction {
  /**
   * Declares a named session: pass the same returned handle to more than one
   * agent's `session` field so both bindings share it.
   * @example `const shared = session("shared-scan"); agent("a", { session: shared }); agent("b", { session: shared });`
   */
  (id: string, opts?: SessionOptions): SessionHandle;
  /** No-sharing lifecycle constant: a fresh session per frame. */
  readonly PerFrame: "per-frame";
  /** No-sharing lifecycle constant: one session for the agent's whole lifetime. */
  readonly Persistent: "persistent";
}

function sessionOf(id: string, opts: SessionOptions = {}): SessionHandle {
  validateSlug(id, "session.id");
  const declaration = compact({ id, description: opts.description });
  return withDeclaration("session", `session:${id}`, declaration, {});
}

export const session: SessionFunction = Object.assign(
  sessionOf,
  {
    PerFrame: "per-frame",
    Persistent: "persistent",
  } as const,
);

/**
 * Lowers an `AgentFields.session`/`AgentTemplateFields.session` binding into
 * its canonical `agent.session` field value: a bare lifecycle constant
 * passes through as itself, and a named `session(...)` handle lowers to its
 * scalar `session:<id>` ref — so two agents bound to the same handle emit
 * byte-identical `session` fields.
 */
export function sessionFieldOf(binding: SessionBinding | undefined): SessionLifecycle | string | undefined {
  return binding === undefined || typeof binding === "string" ? binding : refText(binding, "agent.session");
}
