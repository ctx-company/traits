import type {
  AgentHandle,
  PromptInterpolation,
  SchemaHandle,
  SequenceHandle,
  SignalHandle,
  SlotHandle,
} from "@ctx-traits/cdk";
import { condition, input, schema, sequence } from "@ctx-traits/cdk";

/** The kit's feasibility triage verdict: whether a task is even worth spending a build loop on. */
export type FeasibilityVerdictValue = {
  readonly verdict: "feasible" | "blocked" | "oversized" | "ambiguous";
  readonly evidence: string;
  readonly missing: readonly string[];
  readonly "owner-action": string;
};

/**
 * A built-in triage check run BEFORE any build loop spends anything (0047
 * mechanism 1's agent layer): given the task contract, an agent audits it
 * from four angles — possible, blocked, oversized, ambiguous — verifying
 * with its own tools that everything the task references as existing
 * actually exists in this worktree. Companion to the deterministic
 * blocked-status-marker dispatch preflight (`modules/io/src/
 * dispatch_preflight.rs::blocked_status_marker`): that catches DECLARED
 * blockage for free at dispatch time; this catches UNDECLARED blockage
 * (referenced artifacts absent) that no header ever recorded, at the cost
 * of one cheap frame instead of a whole grind loop parking at exhaustion.
 */
export const feasibilityVerdictSchema: SchemaHandle<FeasibilityVerdictValue> = schema.object(
  "feasibility-verdict",
  {
    verdict: schema.field(schema.enum(["feasible", "blocked", "oversized", "ambiguous"] as const), {
      description:
        "feasible when the task can be built from inside this run as scoped; blocked when it depends on something not landed; oversized when the scope does not honestly fit one run's budget; ambiguous when it lacks a falsifiable Done-when and any implementation would be a guess.",
    }),
    evidence: schema.field(schema.text(), {
      description:
        "What was checked and how: each referenced file, command, recipe, or prerequisite the task names as existing, and whether your tools confirmed it exists. Default to feasible absent proof of a problem — an unverified claim is never grounds for a non-feasible verdict on its own.",
    }),
    missing: schema.field(schema.list(schema.text()), {
      description:
        "The concrete things missing or wrong, one per entry (a referenced file that does not exist, a prerequisite task not landed, the specific ambiguity). Empty when verdict is feasible.",
    }),
    "owner-action": schema.field(schema.text(), {
      description:
        "The one owner action that would clear a non-feasible verdict — land the prerequisite, split the task, or rewrite the Done-when. Empty string (never omitted — always return the key) when verdict is feasible.",
    }),
  },
  {
    description:
      "Typed pre-build feasibility triage: possible, blocked, oversized, or ambiguous, with tool-verified evidence and the owner action that clears a non-feasible verdict.",
  },
);

/**
 * Static prompt doctrine for the feasibility gate's four audit angles.
 * Pure static text (no port/slot/family refs) — safe to splice into any
 * `input.prompt` review-step source via `${FEASIBILITY_DOCTRINE}`.
 */
export const FEASIBILITY_DOCTRINE =
  `Audit the task from exactly four angles before any build work begins. POSSIBLE: can this be done at all from inside a run — the capability, authority, and tooling a shell here actually has? BLOCKED: does it depend on something not landed? For every file, command, fixture, prerequisite API, or recipe the task references as EXISTING, verify with your own tools that it actually exists in this worktree — a task that says "extend X" where X is absent is blocked, not implementable, however small the rest of the scope looks. OVERSIZED: does the scope honestly fit one run's budget, or does it need splitting into smaller tasks first? AMBIGUOUS: does the task state a falsifiable Done-when, or would any implementation here be a guess at what "done" means? Default to feasible: raise a non-feasible verdict only on evidence you actually checked, never on suspicion or scope alone. Return the typed verdict: the tool-checked evidence, every missing thing named as its own entry (empty when feasible), and the one owner action that would clear a non-feasible verdict (empty string when feasible).`;

/** Options for {@link feasibilityGate}. */
export type FeasibilityGateOptions = {
  /** Step/loop identifier prefix. */
  readonly id: string;
  /** The agent running the audit. */
  readonly agent: AgentHandle;
  /** The task identity ref (e.g. `port.task`), interpolated into the prompt. */
  readonly task: PromptInterpolation;
  /** The task's contract ref — its extracted brief, or a resource the agent consults with its own tools (a caller with no extraction step, e.g. quick, passes its task-board resource instead). */
  readonly contract: PromptInterpolation;
  /** Caller-declared slot the audit's typed verdict lands in. */
  readonly output: SlotHandle<FeasibilityVerdictValue>;
  /**
   * What a non-feasible verdict does to the run. `"block"` stops the
   * procedure right at this step, with the verdict slot as the park
   * evidence. `"warn"` (owner ruling 2026-07-31: the audit is advisory for
   * now) records the same typed verdict in the same slot/output port and
   * lets the run continue regardless — the verdict is an audit record the
   * reviewer and the owner read, never a stop.
   */
  readonly mode: "block" | "warn";
  /** The signal(s) the gate emits when it parks a non-feasible verdict. Required in `"block"` mode; unused in `"warn"` mode. */
  readonly onAbort?: SignalHandle | readonly SignalHandle[];
};

/**
 * A 1-iteration triage step builder: a single prompt round auditing the
 * task, wrapped in a `sequence.loop` whose `until`/`abortIf` are exact
 * logical complements of each other (`condition.fieldEquals(output,
 * "verdict", "feasible")` vs. its negation) — mutually exclusive by
 * construction, so no `guard-conflict` diagnostic is reachable and
 * exhaustion (neither guard matching) can never occur. A `feasible`
 * verdict lets the caller's own procedure continue past this step; any
 * other verdict stops the loop right there, with the verdict slot itself
 * as the park evidence — one cheap frame instead of ten grind rounds
 * discovering the same blockage the hard way.
 * @example
 * ```ts
 * const feasibility = slot({ id: "feasibility", schema: feasibilityVerdictSchema, description: "..." });
 * const notFeasible = signal({ id: "not-feasible", description: "..." });
 * const gate = feasibilityGate({ id: "feasibility-check", agent: smart1, task, contract: taskBrief, output: feasibility, onAbort: notFeasible });
 * ```
 */
export function feasibilityGate(options: FeasibilityGateOptions): SequenceHandle {
  const { id, agent, task, contract, output, mode } = options;
  // `input.prompt` is a genuine tagged template: every `${}` interpolation
  // must itself be a typed ref (`PromptInterpolation`), never a plain
  // string — so `FEASIBILITY_DOCTRINE` cannot be spliced in via `${...}`.
  // Calling it with the exact `(strings, ...values)` shape a tagged
  // template desugars to lets the fixed doctrine text live in the literal
  // string segments (where plain text belongs) while `task`/`contract`
  // stay typed, declared interpolations — one source of truth for the
  // doctrine, no duplicated copy.
  const segments: readonly string[] = [
    "Before any build work begins for ",
    ", audit it for feasibility against its contract ",
    ` — read the contract, and everything it references, with your own tools. ${FEASIBILITY_DOCTRINE}`,
  ];
  const strings: TemplateStringsArray = Object.assign([...segments], { raw: segments });
  const checkStep = sequence.prompt(`${id}-check`, {
    title: "Audit feasibility before building",
    agent,
    text: input.prompt(strings, task, contract),
    output,
  });
  // Warn mode: the bare audit step — same prompt, same typed verdict in the
  // same slot — with no loop, no guard, and no stop. The verdict is audit
  // evidence only; the run proceeds whatever it says.
  if (mode === "warn") {
    return checkStep;
  }
  const onAbort = options.onAbort;
  if (onAbort === undefined) {
    throw new Error(`feasibilityGate ${JSON.stringify(id)}: mode "block" requires onAbort`);
  }
  const feasible = condition.fieldEquals(output, "verdict", "feasible");
  return sequence.loop(id, {
    sequence: sequence.linear(`${id}-body`, [checkStep]),
    until: feasible,
    iterations: 1,
    abortIf: condition.not(feasible),
    onAbort,
  });
}
