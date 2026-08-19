/**
 * The step builder (owner ruling 2026-08-11, renamed from `stage` 2026-08-19):
 * a step module declares its parts once — agent, input, output — and
 * `step(fields)` returns a CALLABLE carrying those fields, so a variant body
 * that uses a step as-is reads as one line (`surveyStep("Survey the target")`,
 * `git.status("Check working tree status")`) while the parts stay addressable
 * (`surveyStep.input`, `git.status.output`) for variants that compose or
 * override. Dispatch is by shape: a step with an `agent` registers a prompt
 * step through the agent's own `.prompt` registrar; a step without one
 * registers a command step (its `input` is an `input.command` template).
 *
 * `step` is one identifier with two call shapes: `step(fields)` DECLARES a
 * reusable step, `step.command`/`step.check`/`step.project` PLACE one inline.
 * The canonical, the runtime, the TUI and every build error say "step" too —
 * the authoring word never has to be translated.
 */
import type { AgentHandle, SequenceHandle } from "../handles.js";
import type { CommandTemplateValue } from "../input.js";
import type { CommandSequenceFields, PromptRegistrarOptions } from "../sequence.js";
import { stepRegistrars } from "./registrars.js";
import type { IdOverride } from "./registrars.js";

type CommandRegistrarOptions = Omit<CommandSequenceFields, "id" | "kind" | "title"> & IdOverride;

export type PromptStepFields = {
  readonly agent: AgentHandle;
} & Omit<PromptRegistrarOptions, "id">;

export type CommandStepFields = {
  readonly agent?: never;
  readonly input: CommandTemplateValue;
} & Omit<CommandRegistrarOptions, "id" | "input">;

export type StepFields = PromptStepFields | CommandStepFields;

/**
 * A callable step: invoking it registers the step under `title` (the id
 * derives from it); `overrides` merge over the step's own fields — the escape
 * hatch for a variant that swaps one part while keeping the one-line call site
 * for everything else. The declared fields remain readable as properties.
 */
export type Step<F extends StepFields> = F & {
  (title: string, overrides?: Partial<PromptRegistrarOptions & CommandRegistrarOptions>): SequenceHandle;
};

function declareStep<const F extends StepFields>(fields: F): Step<F> {
  const register = (
    title: string,
    overrides: Partial<PromptRegistrarOptions & CommandRegistrarOptions> = {},
  ): SequenceHandle => {
    const { agent, ...rest } = fields as { readonly agent?: AgentHandle };
    if (agent !== undefined) {
      return agent.prompt(title, { ...rest, ...overrides } as PromptRegistrarOptions);
    }
    return stepRegistrars.command(title, { ...rest, ...overrides } as CommandRegistrarOptions);
  };
  return Object.assign(register, fields) as Step<F>;
}

/**
 * The one step noun: callable with a field bundle to declare a reusable step,
 * and carrying the inline registrars (`step.command`, `step.check`,
 * `step.project`) for the leaf kinds a declaration's shape dispatch does not
 * cover.
 */
export const step = Object.assign(declareStep, stepRegistrars);
