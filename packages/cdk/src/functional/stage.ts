/**
 * The stage builder (owner ruling 2026-08-11): a stage module declares its
 * parts once — agent, input, output — and `stage(fields)` returns those
 * same fields plus a `.step(title)` registrar, so a variant body that uses
 * a stage as-is reads as one line (`survey.step("Survey the target")`,
 * `git.status.step("Check working tree status")`) while the parts stay
 * addressable (`survey.input`, `git.status.output`) for variants that
 * compose or override. Dispatch is by shape: a stage with an `agent`
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

export type Stage<F extends StageFields> = F & {
  /**
   * Registers this stage's step under `title` (the id derives from it).
   * `overrides` merge over the stage's own fields — the escape hatch for
   * a variant that swaps one part (e.g. a flavored input) while keeping
   * the one-line call site for everything else.
   */
  readonly step: (
    title: string,
    overrides?: Partial<PromptRegistrarOptions & CommandStepOptions>,
  ) => SequenceHandle;
};

export function stage<const F extends StageFields>(fields: F): Stage<F> {
  const stepFn = (
    title: string,
    overrides: Partial<PromptRegistrarOptions & CommandStepOptions> = {},
  ): SequenceHandle => {
    const { agent, ...rest } = fields as { readonly agent?: AgentHandle; };
    if (agent !== undefined) {
      return agent.prompt(title, { ...rest, ...overrides } as PromptRegistrarOptions);
    }
    return stepRegistrar.command(title, { ...rest, ...overrides } as CommandStepOptions);
  };
  return { ...fields, step: stepFn };
}
