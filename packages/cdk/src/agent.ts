import { dispatchAgentPrompt } from "./functional/context.js";
import { agentTemplates } from "./generated.js";
import type { AgentTemplateDefinition } from "./generated.js";
import type { AgentHandle, SequenceHandle } from "./handles.js";
import { withDeclaration, withHiddenField } from "./meta.js";
import { collectMany, compact, validateSlug } from "./normalize.js";
import type { PromptRegistrarOptions } from "./sequence.js";
import type { SessionBinding } from "./session.js";
import { sessionFieldOf } from "./session.js";

export interface AgentTemplateFunctions {
  /** Built-in worker template: an implementer that makes a change and reports what it did. @example `agent.worker("worker")` */
  readonly worker: AgentTemplateFunction;
  /** Built-in reviewer template: checks work against a stated standard and reports a verdict. @example `agent.reviewer("reviewer")` */
  readonly reviewer: AgentTemplateFunction;
  /** Built-in planner template: drafts an approach before implementation starts. @example `agent.planner("planner")` */
  readonly planner: AgentTemplateFunction;
  /** Built-in oracle template: consulted for a judgment call rather than for producing or reviewing a change. @example `agent.oracle("oracle")` */
  readonly oracle: AgentTemplateFunction;
  /** Built-in searcher template: gathers information (files, prior art, evidence) rather than changing anything. @example `agent.searcher("searcher")` */
  readonly searcher: AgentTemplateFunction;
}

export interface AgentFields {
  readonly id: string;
  readonly description: string;
  readonly summary?: string;
  /** Standing instructions this role always obeys, delivered through the
   * harness system-prompt channel once per session instead of being repeated
   * in every frame. Put doctrine here and per-step work in the step's prompt.
   * Model-visible like any instruction text: compiled into the model view,
   * audited for hidden content, and covered by the canonical digest. */
  readonly system?: string;
  /** Session binding for this agent's frames — see {@link SessionBinding}. */
  readonly session?: SessionBinding;
}

export interface AgentTemplateFields {
  readonly description?: string;
  readonly summary?: string;
  readonly system?: string;
  /** Session binding for this agent's frames — see {@link SessionBinding}. */
  readonly session?: SessionBinding;
}

export type AgentTemplateFunction = (id: string, fields?: AgentTemplateFields) => AgentHandle;

export interface AgentFunction extends AgentTemplateFunctions {
  (id: string, fields: Omit<AgentFields, "id">): AgentHandle;
  (fields: AgentFields): AgentHandle;
}

function agentOf(fields: AgentFields, extraMeta: Parameters<typeof withDeclaration>[4] = {}): AgentHandle {
  validateSlug(fields.id, "agent.id");
  const declaration = compact({
    id: fields.id,
    description: fields.description,
    summary: fields.summary,
    system: fields.system,
    session: sessionFieldOf(fields.session),
  });
  // `fields.session` lowers to a bare `session:<id>` ref string above, so the
  // handle itself (and the declaration it carries) never reaches the agent's
  // own enumerable surface. Carrying it through `declarations` here is what
  // lets `collectMany` (walking `fields.agent` at the trait root) still find
  // and emit the shared `[[session]]` declaration exactly once, without
  // requiring the author to also list the handle at the trait root.
  const handle = withDeclaration(
    "agent",
    `agent:${fields.id}`,
    declaration,
    {},
    {
      ...extraMeta,
      declarations: collectMany([fields.session]),
    },
  );
  // Non-enumerable so it never serializes into the canonical declaration
  // (same `withHiddenField` pattern as `ChecklistHandle`'s `verdict`/
  // `itemIds`) — the functional layer's `agent.prompt(title, opts)`
  // registrar (0106), reachable from every mint path: `agent(...)`,
  // `agent.worker(...)`/other templates, and the deprecated bare templates.
  return withHiddenField(
    handle,
    "prompt",
    (title: string, opts: PromptRegistrarOptions) => dispatchAgentPrompt(handle, title, opts) as SequenceHandle,
  );
}

/**
 * Declares an abstract agent role: a named place in the procedure that a
 * sequence step delegates to, resolved to a concrete harness/model
 * configuration only at run time.
 *
 * Hand-writing this as JSON means repeating the agent id as a bare string at
 * every step that uses it, with no check that the id you typed matches one
 * actually declared — a typo silently becomes an unresolved reference caught
 * only at synth or run time. `agent(...)` returns a typed handle: pass it to
 * `sequence.prompt`'s `agent` field and a rename or typo fails at `tsc`. The
 * built-in role templates are reachable as `agent.worker`/`agent.reviewer`/
 * `agent.planner`/`agent.oracle`/`agent.searcher`.
 *
 * @param id Agent identifier.
 * @example
 * ```ts
 * const diff = port.input.text({ id: "diff" });
 * const review = slot.text("review");
 * const reviewer = agent("reviewer", {
 *   description: "Reviews a diff for correctness, risk, and scope.",
 *   summary: "Diff reviewer.",
 * });
 * sequence.prompt("review", { agent: reviewer, text: input.prompt`Review ${diff}.`, output: review });
 * ```
 * @see {@link sequence}
 */
function agentFn(id: string, fields: Omit<AgentFields, "id">): AgentHandle;
function agentFn(fields: AgentFields): AgentHandle;
function agentFn(first: string | AgentFields, second?: Omit<AgentFields, "id">): AgentHandle {
  return agentOf(typeof first === "string" ? ({ ...second, id: first } as AgentFields) : first);
}

function agentTemplateOf(
  template: AgentTemplateDefinition,
  id: string,
  fields: AgentTemplateFields,
  extraMeta: Parameters<typeof withDeclaration>[4] = {},
): AgentHandle {
  return agentOf(
    {
      id,
      description: fields.description ?? template.description,
      summary: fields.summary ?? template.summary,
      // exactOptionalPropertyTypes: an explicit `system: undefined` (or
      // `session: undefined`) is a type error, so omit the key entirely when
      // the caller supplied none.
      ...(fields.system === undefined ? {} : { system: fields.system }),
      ...(fields.session === undefined ? {} : { session: fields.session }),
    },
    extraMeta,
  );
}

function agentTemplate(template: AgentTemplateDefinition): AgentTemplateFunction {
  return (id, fields = {}) => agentTemplateOf(template, id, fields);
}

// `Object.assign` inline in the initializer (rather than a separate
// statement after a bare `agentFn` export) is what lets the `AgentFunction`
// annotation below be checked instead of asserted: TypeScript resolves
// `Object.assign`'s overloaded return type to the true intersection of
// `agentFn` and the template map, and only *that* structurally satisfies
// `AgentFunction`'s `AgentTemplateFunctions` surface.
export const agent: AgentFunction = Object.assign(agentFn, {
  worker: agentTemplate(agentTemplates.worker),
  reviewer: agentTemplate(agentTemplates.reviewer),
  planner: agentTemplate(agentTemplates.planner),
  oracle: agentTemplate(agentTemplates.oracle),
  searcher: agentTemplate(agentTemplates.searcher),
} satisfies AgentTemplateFunctions);

/**
 * Mints `count` explicitly ordered `<role>-1..<role>-N` agent handles from a
 * single call, one per seat, in place of hand-numbered mint calls repeated
 * per seat.
 *
 * The id format (`${role}-${n}`, 1-based) is not a new numbering authority —
 * it hardcodes the same format the harness config's seat-alias expansion
 * uses (`expand_role_seats`, `modules/io/src/harness_config.rs:6223`), so
 * canonical output is byte-identical to hand-numbered calls to `mint`.
 *
 * @param mint Any existing agent mint function: `agent`, `agent.reviewer`/
 * `agent.worker`/etc., or a project-specific wrapper like `reviewerRole`.
 * @param role Base role slug (validated the same way as `agent.id`).
 * @param count Number of seats to mint; must be a positive integer.
 * @param args Remaining arguments forwarded to `mint` for every seat.
 * @example
 * ```ts
 * const [smart1, smart2] = seats(reviewerRole, "smart", 2, "Reviews the diff.");
 * ```
 */
// Overloads by forwarded-argument count rather than one variadic signature:
// every agent mint (`agent`, the `agent.*` templates, project wrappers like
// `reviewerRole`) declares its trailing parameters OPTIONAL, which makes TS
// collapse a `...args: A` tuple inferred from the mint to `[]` and reject
// the forwarded fields as an excess argument ("Expected 3 arguments, but
// got 4"). Three arities cover every existing mint shape.
export function seats<H extends AgentHandle>(mint: (id: string) => H, role: string, count: number): readonly H[];
export function seats<H extends AgentHandle, F>(
  mint: (id: string, fields: F) => H,
  role: string,
  count: number,
  fields: F,
): readonly H[];
export function seats<H extends AgentHandle, F, S>(
  mint: (id: string, first: F, second: S) => H,
  role: string,
  count: number,
  first: F,
  second: S,
): readonly H[];
export function seats(
  mint: (id: string, ...args: unknown[]) => AgentHandle,
  role: string,
  count: number,
  ...args: unknown[]
): readonly AgentHandle[] {
  validateSlug(role, "seats.role");
  if (!Number.isInteger(count) || count < 1) {
    throw new Error(`seats.count: expected a positive integer, got ${JSON.stringify(count)}`);
  }
  return Array.from({ length: count }, (_, i) => mint(`${role}-${i + 1}`, ...args));
}
