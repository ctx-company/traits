/**
 * The stage builder (owner ruling 2026-08-11): a stage module declares its
 * parts once — agent, input, output — and `stage(fields)` returns a
 * CALLABLE carrying those fields, so a variant body that uses a stage
 * as-is reads as one line (`surveyStage("Survey the target")`,
 * `git.status("Check working tree status")`) while the parts stay
 * addressable (`surveyStage.input`, `git.status.output`) for variants
 * that compose or override. Dispatch is by shape: a stage with an `agent`
 * registers a prompt step through the agent's own `.prompt` registrar; a
 * stage without one registers a command step (its `input` is an
 * `input.command` template).
 */
import type { AgentHandle, SequenceHandle } from "../handles.js";
import type { CommandTemplateValue } from "../input.js";
import type { CommandSequenceFields, PromptRegistrarOptions } from "../sequence.js";
import { step as stepRegistrar } from "./registrars.js";
import type { IdOverride } from "./registrars.js";

type CommandStepOptions = Omit<CommandSequenceFields, "id" | "kind" | "title"> & IdOverride;

export type PromptStageFields = {
  readonly agent: AgentHandle;
} & Omit<PromptRegistrarOptions, "id">;

export type CommandStageFields = {
  readonly agent?: never;
  readonly input: CommandTemplateValue;
} & Omit<CommandStepOptions, "id" | "input">;

export type StageFields = PromptStageFields | CommandStageFields;

/**
 * A callable stage: invoking it registers the stage's step under `title`
 * (the id derives from it); `overrides` merge over the stage's own fields
 * — the escape hatch for a variant that swaps one part while keeping the
 * one-line call site for everything else. The declared fields remain
 * readable as properties.
 */
export type Stage<F extends StageFields> = F & {
  (
    title: string,
    overrides?: Partial<PromptRegistrarOptions & CommandStepOptions>,
  ): SequenceHandle;
};

export function stage<const F extends StageFields>(fields: F): Stage<F> {
  const register = (
    title: string,
    overrides: Partial<PromptRegistrarOptions & CommandStepOptions> = {},
  ): SequenceHandle => {
    const { agent, ...rest } = fields as { readonly agent?: AgentHandle; };
    if (agent !== undefined) {
      return agent.prompt(title, { ...rest, ...overrides } as PromptRegistrarOptions);
    }
    return stepRegistrar.command(title, { ...rest, ...overrides } as CommandStepOptions);
  };
  return Object.assign(register, fields) as Stage<F>;
}
