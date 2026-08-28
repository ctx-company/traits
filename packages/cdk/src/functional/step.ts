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
import type { AgentHandle, DeclaredSignalWithFields, PromptInterpolation, SequenceHandle } from "../handles.js";
import type { CommandInterpolation, CommandTemplateValue } from "../input.js";
import type {
  AskSequenceFields,
  CheckSequenceFields,
  CommandSequenceFields,
  PromptRegistrarOptions,
  VirtualSlotSurfaceOf,
} from "../sequence.js";
import { stepRegistrars } from "./registrars.js";
import type { IdOverride } from "./registrars.js";

type CommandRegistrarOptions = Omit<CommandSequenceFields, "id" | "kind" | "title"> & IdOverride;
type CheckRegistrarOptions = Omit<CheckSequenceFields, "id" | "kind" | "title"> & IdOverride;
type AskRegistrarOptions = Omit<AskSequenceFields, "id" | "kind" | "title"> & IdOverride;

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

/** A reusable signal-gated question, answered by a human through the normal current-frame submission path — mirrors `AskSequenceFields` (`when`/`output` required, `agent` never). */
export type AskStepFields = Omit<AskRegistrarOptions, "id">;

export type StepFields = PromptStepFields | CommandStepFields | CheckStepFields | AskStepFields;

/**
 * Derives a declared/parameterized step's placement return type from its OWN
 * `output:` field (0253.3), through the SAME conditional type every direct
 * `sequence.*`/`step.*` placement routes through ({@link VirtualSlotSurfaceOf}):
 * a bare-schema (or bare-schema-containing) `output:` value types `.result`/
 * `.results`; anything else (a named slot, no output at all) stays a plain
 * `SequenceHandle` — so a step declared once through `defineStep.x` and a
 * step placed once through `step.x`/`sequence.x` expose the identical
 * addressing surface, with no separate schema/array/plain split to drift.
 *
 * Guarded on a CONCRETE `output` key (`F extends { readonly output: infer
 * RawOutput }`, not the optional-property form): a fields object with no
 * `output` at all does not structurally satisfy a required `output` key, so
 * it falls to the plain-`SequenceHandle` branch instead of inferring
 * `RawOutput` as `never` — `[never] extends [SchemaValue<Value>]` is TRUE
 * (an empty union satisfies every conditional's constraint check), which
 * would otherwise promise a `.result` no runtime attachment ever produces.
 */
export type StepResultOf<F> = F extends { readonly output: infer RawOutput }
  ? SequenceHandle & VirtualSlotSurfaceOf<RawOutput>
  : SequenceHandle;

/**
 * A declared step: calling it registers the step under `title` (the id derives
 * from it). `overrides` merge over the declared fields — the escape hatch for
 * a variant that swaps one part while keeping the one-line call site for
 * everything else.
 */
export type Step<F extends StepFields, R extends SequenceHandle = StepResultOf<F>> = F & {
  (
    title: string,
    overrides?: Partial<PromptRegistrarOptions & CommandRegistrarOptions & CheckRegistrarOptions & AskRegistrarOptions>,
  ): R;
};

/**
 * A declared `check` step places exactly what an inline one does, verdict slot
 * included — the return type is taken FROM the registrar rather than restated,
 * so `gate("…").pass.ok` stays typed and the two forms cannot drift.
 */
export type CheckStep<F extends CheckStepFields> = Step<F, ReturnType<typeof stepRegistrars.check>>;

/**
 * A step declared from a FACTORY instead of a static fields object
 * (0253.3): `defineStep.x((refs) => ({...}))`. Calling it registers the step
 * — the factory runs once per instantiation, with the actual refs passed
 * positionally after the title, so the inferred tuple `P` makes a wrong
 * count or a wrong-kind ref a compile error at the CALL site, not a runtime
 * surprise. Unlike {@link Step}, a parameterized step carries no `overrides`
 * parameter (the refs occupy that position) and no field addressability
 * (`.input`/`.output`) — its fields do not exist until refs are bound.
 * Parameterize instead of overriding; widen only on demonstrated need.
 */
export type ParameterizedStep<P extends readonly unknown[], R = SequenceHandle> = (title: string, ...refs: P) => R;

function definePromptStep<const F extends PromptStepFields>(fields: F): Step<F>;
/**
 * An unannotated factory parameter falls back to {@link PromptInterpolation}
 * and interpolates in `input.prompt`/`text` with no cast. Interpolating it
 * on the OUTPUT side (`output.prompt`) needs an annotated parameter narrowed
 * to what that side accepts (e.g. `SlotHandle<string>`) — `PromptInterpolation`
 * is not assignable to the narrower `OutputPromptInterpolation`.
 */
function definePromptStep<const P extends readonly PromptInterpolation[], const F extends PromptStepFields>(
  factory: (...refs: P) => F,
): ParameterizedStep<P, StepResultOf<F>>;
function definePromptStep(
  fieldsOrFactory: PromptStepFields | ((...refs: readonly PromptInterpolation[]) => PromptStepFields),
): unknown {
  if (typeof fieldsOrFactory === "function") {
    const factory = fieldsOrFactory;
    return (title: string, ...refs: readonly PromptInterpolation[]): SequenceHandle => {
      const { agent, ...rest } = factory(...refs) as unknown as { readonly agent: AgentHandle };
      return agent.prompt(title, rest as PromptRegistrarOptions);
    };
  }
  const fields = fieldsOrFactory;
  const register = (title: string, overrides: Record<string, unknown> = {}): SequenceHandle => {
    const { agent, ...rest } = fields as unknown as { readonly agent: AgentHandle };
    return agent.prompt(title, { ...rest, ...overrides } as PromptRegistrarOptions);
  };
  return Object.assign(register, fields) as unknown as Step<PromptStepFields>;
}

function defineCommandStep<const F extends CommandStepFields>(fields: F): Step<F>;
function defineCommandStep<const P extends readonly CommandInterpolation[], const F extends CommandStepFields>(
  factory: (...refs: P) => F,
): ParameterizedStep<P, StepResultOf<F>>;
function defineCommandStep(
  fieldsOrFactory: CommandStepFields | ((...refs: readonly CommandInterpolation[]) => CommandStepFields),
): unknown {
  if (typeof fieldsOrFactory === "function") {
    const factory = fieldsOrFactory;
    return (title: string, ...refs: readonly CommandInterpolation[]): SequenceHandle =>
      stepRegistrars.command(title, factory(...refs) as unknown as CommandRegistrarOptions);
  }
  const fields = fieldsOrFactory;
  const register = (title: string, overrides: Record<string, unknown> = {}): SequenceHandle =>
    stepRegistrars.command(title, { ...fields, ...overrides } as unknown as CommandRegistrarOptions);
  return Object.assign(register, fields) as unknown as Step<CommandStepFields>;
}

function defineCheckStep<const F extends CheckStepFields>(fields: F): CheckStep<F>;
function defineCheckStep<const P extends readonly CommandInterpolation[], const F extends CheckStepFields>(
  factory: (...refs: P) => F,
): ParameterizedStep<P, ReturnType<typeof stepRegistrars.check>>;
function defineCheckStep(
  fieldsOrFactory: CheckStepFields | ((...refs: readonly CommandInterpolation[]) => CheckStepFields),
): unknown {
  if (typeof fieldsOrFactory === "function") {
    const factory = fieldsOrFactory;
    return (title: string, ...refs: readonly CommandInterpolation[]): ReturnType<typeof stepRegistrars.check> =>
      stepRegistrars.check(title, factory(...refs) as unknown as CheckRegistrarOptions);
  }
  const fields = fieldsOrFactory;
  const register = (title: string, overrides: Record<string, unknown> = {}): ReturnType<typeof stepRegistrars.check> =>
    stepRegistrars.check(title, { ...fields, ...overrides } as unknown as CheckRegistrarOptions);
  return Object.assign(register, fields) as unknown as CheckStep<CheckStepFields>;
}

function defineAskStep<const F extends AskStepFields>(fields: F): Step<F>;
/**
 * Unlike `prompt`/`command`/`check`, an ask's factory parameter is the
 * SIGNAL it is guarded on, not a bare interpolation ref — `when:` needs a
 * signal-typed value to pass through, and `${sig.field}` field reads need
 * the signal's field surface, so an unannotated `PromptInterpolation`
 * fallback would not type either. The factory receives exactly one signal
 * positionally; `defineStep.ask((sig) => ({ when: sig, ... }))`.
 */
function defineAskStep<const Sig extends DeclaredSignalWithFields, const F extends AskStepFields>(
  factory: (signal: Sig) => F,
): ParameterizedStep<[Sig], StepResultOf<F>>;
function defineAskStep(
  fieldsOrFactory: AskStepFields | ((signal: DeclaredSignalWithFields) => AskStepFields),
): unknown {
  if (typeof fieldsOrFactory === "function") {
    const factory = fieldsOrFactory;
    return (title: string, signal: DeclaredSignalWithFields): SequenceHandle =>
      stepRegistrars.ask(title, factory(signal) as unknown as AskRegistrarOptions);
  }
  const fields = fieldsOrFactory;
  const register = (title: string, overrides: Record<string, unknown> = {}): SequenceHandle =>
    stepRegistrars.ask(title, { ...fields, ...overrides } as unknown as AskRegistrarOptions);
  return Object.assign(register, fields) as unknown as Step<AskStepFields>;
}

/**
 * Declares a reusable step. The kind is named rather than inferred, so a
 * `check` is as declarable as a `command` and a reader of the call site knows
 * which of `prompt`/`command`/`check`/`ask` they are looking at.
 *
 * Each also accepts a FACTORY beside the static fields object (0253.3):
 * `defineStep.x((refs) => ({...}))`. An unannotated factory parameter falls
 * back to the reference union each step kind interpolates (`input.prompt`'s
 * {@link PromptInterpolation} for `prompt`, `input.command`'s
 * {@link CommandInterpolation} for `command`/`check`) — bare params
 * type-check and interpolate with no cast; only a parameter needing field
 * access (`signal.foo`) or output-side interpolation needs an annotation.
 * Instantiation passes refs positionally after the title
 * (`commandStep("title", someSlot)`), so a miswired arity or kind is a
 * compile error at the call site.
 */
export const defineStep = {
  /** A reusable agent turn. @example `const review = defineStep.prompt({ agent: reviewer, input, output });` */
  prompt: definePromptStep,
  /** A reusable command step. @example `const status = defineStep.command({ input: input.command\`git status --porcelain\`, output: tree });` */
  command: defineCommandStep,
  /** A reusable check step, verdict slot included. @example `const gate = defineStep.check({ input: input.command\`just test\` });` */
  check: defineCheckStep,
  /** A reusable signal-gated question, answered by a human. @example `const summon = defineStep.ask((sig) => ({ when: sig, input: input.prompt\`${sig.reason}\`, output: schema.text() }));` */
  ask: defineAskStep,
} as const;

/**
 * Places a step inline: `step.command`, `step.check`, `step.project`,
 * `step.ask`. A namespace only — a step's kind is always named at the call
 * site.
 */
export const step = stepRegistrars;
