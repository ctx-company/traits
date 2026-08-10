import type { AgentHandle, PromptInterpolation, SequenceHandle, SignalHandle, SlotHandle } from "@ctx-traits/cdk";
import { condition, input, sequence } from "@ctx-traits/cdk";
import { FEASIBILITY_DOCTRINE } from "../data.ts";
import type { FeasibilityVerdictValue } from "../schema.ts";

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
