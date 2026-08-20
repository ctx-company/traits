/**
 * Declaring a step, and placing one.
 *
 * `defineStep.*` declares a reusable step: the parts are named once — agent,
 * input, output — and the result is a CALLABLE, so a variant that uses the
 * step as-is reads as one line (`surveyStep("Survey the target")`) while the
 * parts stay addressable (`surveyStep.input`) for a variant that overrides one
 * of them. `step.*` places a step inline, where it is used once.
 *
 * Two verbs, because they are two acts. They used to share one: `step(fields)`
 * declared and `step.command(title, fields)` placed, which meant the same
 * noun read as a function at one call site and a namespace at the next. It
 * also hid a gap — `step(fields)` chose its kind by SHAPE, agent present for a
 * prompt and otherwise a command, so a reusable `check` could not be written
 * at all. Naming the kind fixes both.
 *
 * The canonical, the runtime, the TUI and every build error say "step" too —
 * the authoring word never has to be translated.
 */
import type { AgentHandle, SequenceHandle } from "../handles.js";
import type { CommandTemplateValue } from "../input.js";
import type { CheckSequenceFields, CommandSequenceFields, PromptRegistrarOptions } from "../sequence.js";
import { stepRegistrars } from "./registrars.js";
import type { IdOverride } from "./registrars.js";

type CommandRegistrarOptions = Omit<CommandSequenceFields, "id" | "kind" | "title"> & IdOverride;
type CheckRegistrarOptions = Omit<CheckSequenceFields, "id" | "kind" | "title"> & IdOverride;

export type PromptStepFields = {
  readonly agent: AgentHandle;
} & Omit<PromptRegistrarOptions, "id">;

export type CommandStepFields = {
  readonly agent?: never;
  readonly input: CommandTemplateValue;
} & Omit<CommandRegistrarOptions, "id" | "input">;

export type CheckStepFields = {
  readonly agent?: never;
  readonly input: CommandTemplateValue;
} & Omit<CheckRegistrarOptions, "id" | "input">;

export type StepFields = PromptStepFields | CommandStepFields | CheckStepFields;

/**
 * A declared step: calling it registers the step under `title` (the id derives
 * from it). `overrides` merge over the declared fields — the escape hatch for
 * a variant that swaps one part while keeping the one-line call site for
 * everything else.
 */
export type Step<F extends StepFields, R extends SequenceHandle = SequenceHandle> = F & {
  (title: string, overrides?: Partial<PromptRegistrarOptions & CommandRegistrarOptions & CheckRegistrarOptions>): R;
};

/**
 * A declared `check` step places exactly what an inline one does, verdict slot
 * included — the return type is taken FROM the registrar rather than restated,
 * so `gate("…").pass.ok` stays typed and the two forms cannot drift.
 */
export type CheckStep<F extends CheckStepFields> = Step<F, ReturnType<typeof stepRegistrars.check>>;

/**
 * Declares a reusable step. The kind is named rather than inferred, so a
 * `check` is as declarable as a `command` and a reader of the call site knows
 * which of the three they are looking at.
 */
export const defineStep = {
  /** A reusable agent turn. @example `const review = defineStep.prompt({ agent: reviewer, input, output });` */
  prompt<const F extends PromptStepFields>(fields: F): Step<F> {
    const register = (title: string, overrides: Record<string, unknown> = {}): SequenceHandle => {
      const { agent, ...rest } = fields as unknown as { readonly agent: AgentHandle };
      return agent.prompt(title, { ...rest, ...overrides } as PromptRegistrarOptions);
    };
    return Object.assign(register, fields) as unknown as Step<F>;
  },
  /** A reusable command step. @example `const status = defineStep.command({ input: input.command\`git status --porcelain\`, output: tree });` */
  command<const F extends CommandStepFields>(fields: F): Step<F> {
    const register = (title: string, overrides: Record<string, unknown> = {}): SequenceHandle =>
      stepRegistrars.command(title, { ...fields, ...overrides } as unknown as CommandRegistrarOptions);
    return Object.assign(register, fields) as unknown as Step<F>;
  },
  /** A reusable check step, verdict slot included. @example `const gate = defineStep.check({ input: input.command\`just test\` });` */
  check<const F extends CheckStepFields>(fields: F): CheckStep<F> {
    const register = (
      title: string,
      overrides: Record<string, unknown> = {},
    ): ReturnType<typeof stepRegistrars.check> =>
      stepRegistrars.check(title, { ...fields, ...overrides } as unknown as CheckRegistrarOptions);
    return Object.assign(register, fields) as unknown as CheckStep<F>;
  },
} as const;

/**
 * Places a step inline: `step.command`, `step.check`, `step.project`. A
 * namespace only — a step's kind is always named at the call site.
 */
export const step = stepRegistrars;
