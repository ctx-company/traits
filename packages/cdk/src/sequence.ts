import { condition, defaultOutputPaths, lowerCheckGuard, normalizeGuard } from "./condition.js";
import type { BranchCheckValue, GuardValue } from "./condition.js";
import type {
  CanonicalBranchFailureEntry,
  CanonicalExhaustionTarget,
  CanonicalFailureRoute,
  CanonicalFailureTarget,
  CanonicalGuardExpr,
  CanonicalJoinPolicy,
  CanonicalOutputSink,
  CanonicalProjection,
  CanonicalSequenceItem,
  CanonicalSignalEmissionRule,
  JsonObject,
  JsonValue,
  WriteOperation,
} from "./generated.js";
import type {
  AgentHandle,
  InstructionOutputHandle,
  OptionalSlotRead,
  OutputSinkHandle,
  OutputTemplateHandle,
  PortHandle,
  PromptHandle,
  PromptTemplate,
  RefHandle,
  ResourceHandle,
  SchemaHandle,
  SequenceHandle,
  SequenceLinearHandle,
  SlotHandle,
} from "./handles.js";
import { input } from "./input.js";
import type { CommandTemplateValue, OptionalSlotInputValue } from "./input.js";
import {
  attachInstructionOutput,
  attachMeta,
  claimOutputTemplateAuthorship,
  instructionOutputContent,
  isInstructionOutputHandle,
  isOutputTemplateHandle,
  metaOf,
  outputSinkDeclaration,
  outputTemplateContent,
  recordDiagnostic,
  withDeclaration,
  withHiddenField,
  withMeta,
} from "./meta.js";
import {
  collectMany,
  compact,
  compactAs,
  normalizeValue,
  REF_TOKEN_PATTERN,
  sourceMapForSequenceItems,
  tokenizeShellLiteral,
  uniqueInOrder,
  validateSlug,
} from "./normalize.js";
import type { Mutable } from "./normalize.js";
import { OUTPUT_RENDER_V1 } from "./output.js";
import { refText } from "./ref.js";
import { schema } from "./schema.js";
import { isLiteralProjectionSource, slot } from "./slot.js";
import { operation } from "./slot.js";
import type { LiteralProjectionSource } from "./slot.js";

/** Loop exhaustion policy spellings (P448): `sequence.flow.Continue`/`sequence.flow.Abort` evaluate to these. */
export type ExhaustionPolicy = "continue" | "abort";
/** A single loop-exhaustion signal ref, authored the same way as `onComplete`. */
type ExhaustionSignalValue = string | RefHandle<"signal">;
type SequenceRefValue = string | RefHandle<"sequence"> | SequenceLinearHandle;
type BranchArmValue = SequenceRefValue | readonly [SequenceHandle, ...SequenceHandle[]];
type SequenceScope = { readonly kind: "procedure"; } | { readonly kind: "sequence"; readonly id: string; };
/** A resource input included only when `when` matches at readiness (P290). */
export type ConditionalResourceInputValue<Value = unknown> = {
  readonly resource: ResourceHandle<Value>;
  readonly when: GuardValue;
};
export type SequenceInputValue<Value = unknown> =
  | PortHandle<Value>
  | SlotHandle<Value>
  | ResourceHandle<Value>
  | ConditionalResourceInputValue<Value>
  | OptionalSlotInputValue<Value>;
export type SequenceOutputValue<Value = unknown> =
  | PortHandle<Value>
  | SlotHandle<Value>
  | SchemaHandle<Value>
  | OutputSinkHandle<Value>
  | InstructionOutputHandle<Value>
  | OptionalSlotRead<Value>
  | OutputTemplateHandle;
export type ArgvItem = string | SlotHandle | PortHandle | ResourceHandle;
type SignalOutputValue = string | RefHandle<"signal"> | {
  readonly signal: string | RefHandle<"signal">;
  readonly when: GuardValue;
};
export type FailureRoute = {
  readonly step: string;
  readonly signal?: string | RefHandle<"signal">;
};
type FailureTargetValue = string | RefHandle<"signal"> | FailureRoute;

interface SequenceCommonFields {
  readonly id: string;
  readonly title?: string;
  readonly description?: string;
  readonly agent?: AgentHandle;
  readonly input?: SequenceInputValue | readonly SequenceInputValue[];
  readonly output?: SequenceOutputValue | readonly SequenceOutputValue[];
  readonly timeoutMs?: number;
  readonly idleTimeoutMs?: number;
  readonly timeout?: number;
  readonly successExitCode?: number | readonly number[];
  readonly format?: string | readonly string[];
  readonly onComplete?: SignalOutputValue | readonly SignalOutputValue[];
  readonly onFailure?: FailureTargetValue;
  /** Single-step entry guard: this step is skipped unless `when` matches
   * (0106). Composes with the object layer's existing `branch`/`ask` guard
   * spellings without replacing either — a `branch` item still spells its
   * guard as `if`, and an `ask` item's `when` stays a signal ref; this field
   * is the single-step-no-nesting home for every other step kind. */
  readonly when?: GuardValue;
}
type PromptSequenceFields = Omit<SequenceCommonFields, "input"> & {
  readonly kind?: "prompt";
  readonly prompt: PromptHandle | PromptTemplate;
  /** See {@link CommandSequenceFields.include}. */
  readonly include?: SequenceInputValue | readonly SequenceInputValue[];
  /** @deprecated Declare the prompt body via `input.prompt` in the `input:` field, or the `text:` field — this legacy list stays honored and merges before `include:`. */
  readonly input?: SequenceInputValue | readonly SequenceInputValue[];
  readonly text?: never;
  readonly cmd?: never;
  readonly argv?: never;
  readonly sequence?: never;
};
type TextPromptSequenceFields = Omit<SequenceCommonFields, "input"> & {
  readonly kind?: "prompt";
  /** @deprecated Use the `input:` field with `input.prompt` instead. */
  readonly text: PromptTemplate;
  /** See {@link CommandSequenceFields.include}. */
  readonly include?: SequenceInputValue | readonly SequenceInputValue[];
  /** @deprecated Declare non-interpolated dependencies via `include:` — this legacy list stays honored and merges before it. */
  readonly input?: SequenceInputValue | readonly SequenceInputValue[];
  readonly prompt?: never;
  readonly cmd?: never;
  readonly argv?: never;
  readonly sequence?: never;
};
/**
 * The preferred prompt-step shape: `input:` carries the compiled prompt body
 * itself (an `input.prompt` tagged-template value), and
 * non-interpolated dependencies live in `include:` — mirroring the
 * `input.command`/`include:` command-step surface.
 */
type InputPromptSequenceFields = Omit<SequenceCommonFields, "input"> & {
  readonly kind?: "prompt";
  readonly input: PromptTemplate;
  /** See {@link CommandSequenceFields.include}. */
  readonly include?: SequenceInputValue | readonly SequenceInputValue[];
  readonly text?: never;
  readonly prompt?: never;
  readonly cmd?: never;
  readonly argv?: never;
  readonly sequence?: never;
};
/**
 * The options `agent.prompt(title, opts)` accepts (0106): every
 * `sequence.prompt` field except `id`/`kind`/`agent`/`title`, which the
 * registrar mints from the agent handle and the call's own `title`.
 */
export type PromptRegistrarOptions =
  & Omit<PromptSequenceFields | TextPromptSequenceFields | InputPromptSequenceFields, "id" | "kind" | "agent" | "title">
  & { readonly output?: SequenceOutputValue | readonly SequenceOutputValue[]; };
/** A human-owned prompt. The required signal guard decides whether the frame
 * is exposed; its one local slot output is supplied through `session frame set`. */
type AskSequenceFields =
  & Omit<SequenceCommonFields, "agent" | "format" | "onComplete" | "onFailure" | "input" | "when">
  & {
    readonly kind: "ask";
    readonly when: RefHandle<"signal">;
    readonly output: SlotHandle;
    /** See {@link CommandSequenceFields.include}. */
    readonly include?: SequenceInputValue | readonly SequenceInputValue[];
    readonly agent?: never;
    readonly text?: never;
    readonly cmd?: never;
    readonly argv?: never;
    readonly sequence?: never;
    readonly format?: never;
    readonly onComplete?: never;
    readonly onFailure?: never;
  }
  & (
    | {
      readonly prompt: PromptHandle | PromptTemplate;
      /** @deprecated Declare the prompt body via `input.prompt` in the `input:` field instead. */
      readonly input?: SequenceInputValue | readonly SequenceInputValue[];
    }
    | {
      readonly input: PromptTemplate;
      readonly prompt?: never;
    }
  );
export type CommandSequenceFields =
  & Omit<SequenceCommonFields, "input">
  & {
    readonly kind?: "command";
    readonly executableDigestFrom?: PortHandle<string> | SlotHandle<string>;
    /** Non-interpolated dependencies for this step — including
     * `input.optional(...)` markers — that the step's argv does not itself
     * reference. Honored on every argv source (`cmd`, `argv`, `argvFrom`,
     * and an `input:` command template alike). A legacy `input:` dependency
     * list on a `cmd`/`argv`/`argvFrom` step (the landed P447 surface) stays
     * honored and merges BEFORE `include:` entries. */
    readonly include?: SequenceInputValue | readonly SequenceInputValue[];
    readonly prompt?: never;
    readonly text?: never;
    readonly sequence?: never;
  }
  & (
    | {
      readonly cmd: string;
      readonly argv?: never;
      readonly argvFrom?: never;
      /** @deprecated Declare non-interpolated dependencies via `include:` — this legacy list stays honored and merges before it. */
      readonly input?: SequenceInputValue | readonly SequenceInputValue[];
    }
    | {
      readonly argv: readonly ArgvItem[];
      readonly cmd?: never;
      readonly argvFrom?: never;
      /** @deprecated Declare non-interpolated dependencies via `include:` — this legacy list stays honored and merges before it. */
      readonly input?: SequenceInputValue | readonly SequenceInputValue[];
    }
    | {
      readonly argvFrom: PortHandle<string[]>;
      readonly cmd?: never;
      readonly argv?: never;
      /** @deprecated Declare non-interpolated dependencies via `include:` — this legacy list stays honored and merges before it. */
      readonly input?: SequenceInputValue | readonly SequenceInputValue[];
    }
    | {
      readonly input: CommandTemplateValue;
      readonly cmd?: never;
      readonly argv?: never;
      readonly argvFrom?: never;
    }
  );
/**
 * One closed source-to-destination write in an atomic `project` step. The
 * source is either an existing local slot, or a canonical typed literal
 * declared with `operation.literal(...)` (P431) — a literal source must not
 * be combined with `field`.
 */
export type ProjectProjection = {
  readonly source: SlotHandle | LiteralProjectionSource;
  readonly field?: string;
  readonly destination: SlotHandle | OutputSinkHandle;
  readonly operation?: "replace" | "append" | "increment";
};
export type ProjectSequenceFields = {
  readonly kind: "project";
  readonly id: string;
  readonly title?: string;
  readonly description?: string;
  readonly projections: readonly ProjectProjection[];
  readonly prompt?: never;
  readonly text?: never;
  readonly cmd?: never;
  readonly argv?: never;
  readonly argvFrom?: never;
  readonly sequence?: never;
  readonly agent?: never;
  readonly input?: never;
  readonly output?: never;
  readonly timeoutMs?: never;
  readonly idleTimeoutMs?: never;
  readonly timeout?: never;
  readonly successExitCode?: never;
  readonly format?: never;
  readonly onComplete?: never;
  readonly onFailure?: never;
};

/** Options for the composed human approval gate. The gate is lowered entirely
 * to ask, branch, loop, and project items; it introduces no runtime kind. */
export type GateOptions = {
  readonly proposal: SlotHandle<string>;
  readonly when: RefHandle<"signal">;
  readonly rounds: number;
  /** Optional caller-owned replacement path, used only for an `edit` decision. */
  readonly editStep?: SequenceHandle;
};
/**
 * A `check` step: like `command`, but its single declared output is a
 * `schema:boolean` slot that always receives a verdict — `true` on a
 * successful exit, `false` on a non-success exit or timeout. A failing check
 * never routes through ordinary command failure handling: the loop simply
 * continues and re-evaluates its `until` guard against the latest verdict.
 */
export type CheckSequenceFields =
  & Omit<SequenceCommonFields, "format" | "output" | "onFailure" | "input">
  & {
    readonly kind: "check";
    readonly prompt?: never;
    readonly text?: never;
    readonly sequence?: never;
    readonly format?: never;
    readonly output: SlotHandle<boolean>;
    /** See {@link CommandSequenceFields.include}. */
    readonly include?: SequenceInputValue | readonly SequenceInputValue[];
    /** A check's false verdict is a normal accepted value, never a
     * rejected-output route — there is no failure to route. */
    readonly onFailure?: never;
  }
  & (
    | {
      readonly cmd: string;
      readonly argv?: never;
      /** @deprecated Declare non-interpolated dependencies via `include:` — this legacy list stays honored and merges before it. */
      readonly input?: SequenceInputValue | readonly SequenceInputValue[];
    }
    | {
      readonly argv: readonly ArgvItem[];
      readonly cmd?: never;
      /** @deprecated Declare non-interpolated dependencies via `include:` — this legacy list stays honored and merges before it. */
      readonly input?: SequenceInputValue | readonly SequenceInputValue[];
    }
    | { readonly input: CommandTemplateValue; readonly cmd?: never; readonly argv?: never; }
  );
type LinearSequenceFields = SequenceCommonFields & {
  readonly kind: "sequence";
  readonly sequence: SequenceRefValue;
  /** See {@link CommandSequenceFields.include}. */
  readonly include?: SequenceInputValue | readonly SequenceInputValue[];
  readonly prompt?: never;
  readonly text?: never;
  readonly cmd?: never;
  readonly argv?: never;
  readonly onFailure?: never;
};
export type LoopSequenceFields = SequenceCommonFields & {
  readonly kind: "loop";
  /** The loop body as a named-reusable ref. Exactly one of `sequence`/`body` is required. */
  readonly sequence?: SequenceRefValue;
  /** The loop body inline: materializes as a nested sequence auto-named `<step-id>-body`. Exactly one of `sequence`/`body` is required. */
  readonly body?: readonly SequenceHandle[];
  /** The iteration bound. Omit it (alongside `maxIterations`) to declare an
   * unbounded loop — one that never exhausts and exits only via `until`/
   * `abortIf`, which is then required. A large number picked to approximate
   * "never" is refused authoring, not honored: say unbounded, don't fake it. */
  readonly iterations?: number | PortHandle<number>;
  readonly maxIterations?: number;
  readonly until?: BranchCheckValue;
  readonly abortIf?: BranchCheckValue;
  /** Exhaustion policy. Omitted (the default) or `"continue"`: spending the
   * full iteration budget without matching `until` is a normal outcome — the
   * procedure proceeds to the next step, which is responsible for reading
   * the loop's output rather than assuming it succeeded. A budget is a limit
   * on effort; reaching it means the work is unfinished, not that the run is
   * broken. One or more signal refs may be declared alongside continuing:
   * they are emitted as recorded evidence a later guard may read (or
   * ignore) — a bare signal is equivalent to `"continue"` plus that signal.
   * `"abort"`: stop the run blocked at exhaustion instead, for procedures
   * where an unmet exit condition invalidates everything after it; no
   * signal is emitted. A run started with `--strict-loops` blocks every
   * loop regardless of its declared policy, and suppresses signal emission
   * even when the declaration names one. */
  readonly onExhausted?: ExhaustionPolicy | ExhaustionSignalValue | readonly ExhaustionSignalValue[];
  /** Names the signal(s) to emit when `abortIf` is the arm that halts the
   * loop, so the trait's own terminal reason (e.g.
   * `recurring-blocker-unresolved`) is distinguishable from exhaustion in
   * the ledger, the receipt, and `run-status` — the runtime still reports
   * the accurate mechanism (`abort-if-matched`) alongside it. Requires
   * `abortIf`; a policy keyword (`"continue"`/`"abort"`) is meaningless here
   * since a `abort-if` match always halts the loop, and is rejected. */
  readonly onAbort?: ExhaustionSignalValue | readonly ExhaustionSignalValue[];
  /** A loop has no failure of its own to route: spending the budget without
   * matching `until` is exhaustion, which `onExhausted` governs, and the items
   * inside the body route their own failures. */
  readonly onFailure?: never;
  /** See {@link CommandSequenceFields.include}. */
  readonly include?: SequenceInputValue | readonly SequenceInputValue[];
  readonly prompt?: never;
  readonly text?: never;
  readonly cmd?: never;
  readonly argv?: never;
  readonly argvFrom?: never;
};
export type ForEachSequenceFields = SequenceCommonFields & {
  readonly kind: "for-each";
  /** The per-item body as a named-reusable ref. Exactly one of `sequence`/`body` is required. */
  readonly sequence?: SequenceRefValue;
  /** The per-item body inline: materializes as a nested sequence auto-named `<step-id>-body`. Exactly one of `sequence`/`body` is required. */
  readonly body?: readonly SequenceHandle[];
  readonly over: string | SlotHandle | RefHandle;
  readonly item: string | SlotHandle | RefHandle;
  readonly limit?: number;
  readonly maxItems?: number;
  readonly concurrent?: boolean;
  /** See {@link CommandSequenceFields.include}. */
  readonly include?: SequenceInputValue | readonly SequenceInputValue[];
  readonly prompt?: never;
  readonly text?: never;
  readonly cmd?: never;
  readonly argv?: never;
};
/**
 * Barrier join policy for a `parallel` panel (P264). Omitted is behaviorally
 * identical to `{ policy: "collect-in-order" }` — the P263 barrier.
 */
export type ParallelJoinOption =
  | { readonly policy: "collect-in-order"; }
  | {
    readonly policy: "reduce-merge";
    readonly destination: string | SlotHandle;
    readonly source: string | SlotHandle;
    readonly operation: "merge" | Extract<WriteOperation, { readonly "set-field": string; }>;
  }
  | {
    readonly policy: "quorum-verdict";
    readonly destination: string | SlotHandle;
    readonly source: string | SlotHandle;
    readonly guard: GuardValue;
  };
export type ParallelBranchFailurePolicy = "skip" | "park" | "panel-fail";
export type ParallelBranchFailureEntry = {
  readonly branch: SequenceRefValue;
  readonly onFailure: ParallelBranchFailurePolicy;
};
export type ParallelOptions = {
  readonly join?: ParallelJoinOption;
  readonly branchFailure?: readonly ParallelBranchFailureEntry[];
  readonly onFailure?: FailureTargetValue;
  /** See {@link CommandSequenceFields.include}. */
  readonly include?: SequenceInputValue | readonly SequenceInputValue[];
};
export type ParallelSequenceFields =
  & Omit<
    SequenceCommonFields,
    | "agent"
    | "input"
    | "output"
    | "timeoutMs"
    | "idleTimeoutMs"
    | "timeout"
    | "successExitCode"
    | "format"
    | "onComplete"
    | "onFailure"
    | "when"
  >
  & {
    readonly kind: "parallel";
    readonly branches: readonly SequenceRefValue[];
    readonly join?: ParallelJoinOption;
    readonly branchFailure?: readonly ParallelBranchFailureEntry[];
    readonly onFailure?: FailureTargetValue;
    /** See {@link CommandSequenceFields.include}. */
    readonly include?: SequenceInputValue | readonly SequenceInputValue[];
    readonly prompt?: never;
    readonly text?: never;
    readonly cmd?: never;
    readonly argv?: never;
    readonly sequence?: never;
    readonly agent?: never;
    readonly input?: never;
    readonly output?: never;
    readonly timeoutMs?: never;
    readonly idleTimeoutMs?: never;
    readonly timeout?: never;
    readonly successExitCode?: never;
    readonly format?: never;
    readonly onComplete?: never;
  };
export type BranchSequenceFields =
  & Omit<
    SequenceCommonFields,
    | "agent"
    | "input"
    | "output"
    | "timeoutMs"
    | "idleTimeoutMs"
    | "timeout"
    | "successExitCode"
    | "format"
    | "onComplete"
    | "onFailure"
    | "when"
  >
  & {
    readonly kind: "branch";
    readonly if: GuardValue;
    readonly then: BranchArmValue;
    readonly otherwise?: BranchArmValue;
    /** See {@link CommandSequenceFields.include}. */
    readonly include?: SequenceInputValue | readonly SequenceInputValue[];
    readonly prompt?: never;
    readonly text?: never;
    readonly cmd?: never;
    readonly argv?: never;
    readonly sequence?: never;
    readonly agent?: never;
    readonly input?: never;
    readonly output?: never;
    readonly timeoutMs?: never;
    readonly idleTimeoutMs?: never;
    readonly timeout?: never;
    readonly successExitCode?: never;
    readonly format?: never;
    readonly onComplete?: never;
    readonly onFailure?: never;
  };
/**
 * `sequence.branch`'s fields: `check`/`success`/`failure` spellings of
 * `sequence.when`'s `if`/`then`/`otherwise` (CDK grammar v2 rule 6a), with
 * `check` additionally accepting an unplaced `sequence.check(...)` step as a
 * one-shot gate (see {@link BranchCheckValue}). Unlike `when`'s `then`,
 * `success`/`failure` are each optional — at least one is required, checked
 * at runtime rather than with an unreadable type-level xor.
 */
export type GateBranchFields =
  & Omit<BranchSequenceFields, "if" | "then" | "otherwise">
  & {
    readonly check: BranchCheckValue;
    readonly success?: BranchArmValue;
    readonly failure?: BranchArmValue;
  };
export type SequenceFields =
  | PromptSequenceFields
  | TextPromptSequenceFields
  | InputPromptSequenceFields
  | AskSequenceFields
  | CommandSequenceFields
  | CheckSequenceFields
  | ProjectSequenceFields
  | LinearSequenceFields
  | LoopSequenceFields
  | ForEachSequenceFields
  | BranchSequenceFields
  | ParallelSequenceFields;

/**
 * Sequence builder call signatures: one step of a procedure's execution
 * order, discriminated by `kind` — a prompt to an agent, a shell command, or
 * one of the control-flow kinds (`loop`, `for-each`, `when`/branch,
 * `parallel`) that arrange other steps.
 *
 * The single-line `@example`s on the individual methods below all reuse this
 * declared context:
 * @param id The sequence or control-flow identifier.
 * @example
 * ```ts
 * const reviewer = agent({ id: "reviewer", description: "Reviews a diff." });
 * const diff = port.input.text({ id: "diff" });
 * const focus = port.input.text({ id: "focus" });
 * const review = slot.text("review");
 * const verdict1 = slot.text("verdict1");
 * const verdict2 = slot.text("verdict2");
 * const reviewStep = sequence.prompt("review", {
 *   agent: reviewer,
 *   text: input.prompt`Review ${diff} with a focus on ${focus}.`,
 *   output: review,
 * });
 * const applyFixesStep = sequence.command({ id: "apply-fixes", cmd: "apply-fixes" });
 * const reviewOne = sequence.linear("review-one", [reviewStep]);
 * const files = slot.texts("files");
 * const currentFile = slot.text("current-file");
 * const mergeStep = sequence.command({ id: "merge", cmd: "git merge --no-ff" });
 * const reviseStep = sequence.command({ id: "revise", cmd: "revise" });
 * ```
 * @see {@link SequenceFields}
 */
export interface SequenceFunction {
  /**
   * Declares a named, reusable sequence body from an ordered list of steps —
   * the shape `sequence.loop` and `sequence.forEach` run repeatedly, and
   * what a `sequence.linear` handle refers to by ref.
   * @example `sequence.linear("refine-work", [reviewStep, applyFixesStep])`
   */
  linear<Input = unknown, Output = unknown>(
    id: string,
    items: readonly SequenceHandle<Input, Output>[],
    options?: { readonly description?: string; },
  ): SequenceLinearHandle<Input, Output>;
  /**
   * Declares a prompt sequence step: sends a prompt to `agent` and, if
   * `output` is set, binds the model's structured/text result to a slot.
   * @example
   * ```ts
   * sequence.prompt("review", {
   *   agent: reviewer,
   *   text: input.prompt`Review ${diff} with a focus on ${focus}.`,
   *   output: review,
   * });
   * ```
   */
  prompt<Input, Output = unknown>(
    fields: Omit<InputPromptSequenceFields, "input" | "output"> & {
      readonly input: PromptTemplate<Input>;
      readonly output?: SequenceOutputValue<Output> | readonly SequenceOutputValue<Output>[];
    },
  ): SequenceHandle<Input, Output>;
  prompt<Input, Output = unknown>(
    fields: Omit<PromptSequenceFields, "prompt" | "input" | "output"> & {
      readonly prompt: PromptTemplate<Input>;
      readonly input?: SequenceInputValue<Input> | readonly SequenceInputValue<Input>[];
      readonly output?: SequenceOutputValue<Output> | readonly SequenceOutputValue<Output>[];
    },
  ): SequenceHandle<Input, Output>;
  prompt<Input, Output = unknown>(
    fields: Omit<TextPromptSequenceFields, "text" | "input" | "output"> & {
      readonly text: PromptTemplate<Input>;
      readonly input?: SequenceInputValue<Input> | readonly SequenceInputValue<Input>[];
      readonly output?: SequenceOutputValue<Output> | readonly SequenceOutputValue<Output>[];
    },
  ): SequenceHandle<Input, Output>;
  prompt(
    id: string,
    fields: Omit<PromptSequenceFields | TextPromptSequenceFields | InputPromptSequenceFields, "id" | "kind">,
  ): SequenceHandle;
  prompt(fields: PromptSequenceFields | TextPromptSequenceFields | InputPromptSequenceFields): SequenceHandle;
  /**
   * Declares a shell-command sequence step: runs `cmd` (parsed as a plain
   * argv split, no shell), an explicit `argv`, or `argvFrom`, a list-valued
   * input port resolved once into concrete argv for the command frame. Slots
   * are not accepted for `argvFrom`, so model output cannot select an
   * executable. Structured slot/port values are rendered as compact JSON in
   * one argv element. Useful for the deterministic steps around model work —
   * staging, committing, running a gate — where an agent has nothing to
   * decide.
   * @example `sequence.command("commit", { argv: ["git", "commit", "-m", "Apply review fixes"] })`
   */
  command(id: string, fields: Omit<CommandSequenceFields, "id" | "kind">): SequenceHandle;
  /** @deprecated Pass the id positionally: `sequence.command(id, fields)`. */
  command(fields: CommandSequenceFields): SequenceHandle;
  /**
   * Declares a check step: runs `cmd`/`argv` like `sequence.command`, but
   * writes a pass/fail boolean verdict to `output` instead of the command's
   * raw result — `true` on a successful exit, `false` on a non-success exit
   * or timeout. Meant to sit inside a `sequence.loop`'s `until` guard: a
   * failing check does not stop the loop, it just means `until` stays false
   * for another round.
   * @example
   * ```ts
   * const markerReady = slot.boolean("marker-ready");
   * const checkStep = sequence.check("check-marker", { cmd: "test -f marker", output: markerReady });
   * ```
   * The returned handle also exposes its declared verdict slot as `.pass`,
   * so it can be passed directly as `sequence.branch`'s `check` field
   * without re-declaring the guard by hand.
   */
  check(
    id: string,
    fields: Omit<CheckSequenceFields, "id" | "kind">,
  ): SequenceHandle & { readonly pass: SlotHandle<boolean>; };
  /** Declares a signal-gated question that is answered by a human through the
   * normal current-frame submission path. */
  ask(id: string, fields: Omit<AskSequenceFields, "id" | "kind">): SequenceHandle;
  /**
   * Repeats a producing step until a human approves or edits its proposal.
   * `reject` leaves the gate unsettled, so the bounded blocking loop reruns
   * the producer. The default edit path projects `decision.edit` with Replace.
   */
  gate(producer: SequenceHandle, options: GateOptions): SequenceHandle;
  /**
   * Applies closed, deterministic slot projections as one atomic internal
   * runtime leaf. Every source is read from the same pre-step snapshot;
   * every destination is validated before any write commits. A projection
   * may replace a typed slot, append one compatible list element, or increment
   * a numeric counter. It never creates an agent frame or command evidence.
   *
   * Passing `operation.over(destination, operation.Append)` as `destination`
   * preserves that operation in both the derived output declaration and the
   * projection entry.
   * @example `sequence.project("keep-best", { projections: [{ source: candidate, field: "metric", destination: bestMetric }] })`
   */
  project(
    id: string,
    fields: Omit<ProjectSequenceFields, "id" | "kind">,
  ): SequenceHandle;
  /**
   * Declares a bounded loop: repeats a `sequence.linear` body up to a fixed
   * iteration budget, checking an exit condition after each round.
   *
   * Writing this by hand as JSON risks a loop with no way out (no bound) or
   * one whose exit condition can never fire (referencing the wrong slot) —
   * both invisible until the trait actually runs. `sequence.loop` takes the
   * budget and the guard as typed fields checked against the same slot/
   * condition handles the rest of the trait uses.
   *
   * - `iterations` (aliased `maxIterations`) is the iteration BOUND: the
   *   hard ceiling on how many times the body can run. It may be a literal or
   *   an integer input-port handle; the latter emits `max-iterations-from` and
   *   is resolved once at loop entry. This is not a target
   *   or a suggestion — it is the most rounds the loop will ever spend.
   * - `until` is the guard for SUCCESSFUL completion: the loop re-checks it
   *   after every round and stops early, successfully, the first round it
   *   is satisfied. A round that doesn't satisfy `until` simply continues to
   *   the next round (or to `abortIf`, or to exhaustion).
   * - `onExhausted` governs what happens if the iteration bound is reached
   *   WITHOUT `until` ever succeeding: `"continue"` (the default) treats
   *   running out of budget as a normal outcome — the loop's own output is
   *   whatever the last round produced, and the procedure moves on to the
   *   next step, which is responsible for reading that output rather than
   *   assuming success. One or more signal refs may be declared instead of
   *   (or alongside) `"continue"`: they are emitted as recorded evidence a
   *   later guard may read, or ignore. `"abort"` stops the run at exhaustion
   *   instead, for procedures where an unmet `until` invalidates everything
   *   after it — no signal is emitted in that case.
   *
   * Crucially, none of this is agent-controlled: the model inside the loop
   * body never decides to keep going or to stop — the HOST/RUNTIME evaluates
   * `until`/`abortIf` against the loop's own slots after each round and
   * enforces the `iterations` bound and `onExhausted` policy on its own.
   * The body's prompts can only produce the values the guards read; they
   * cannot request another round or end the loop themselves.
   *
   * @example
   * ```ts
   * sequence.loop("refinement-loop", {
   *   sequence: sequence.linear("refine-work", [reviewStep, applyFixesStep]),
   *   until: condition.all([
   *     condition.fieldEquals(verdict1, "status", "approved"),
   *     condition.fieldEquals(verdict2, "status", "approved"),
   *   ]),
   *   iterations: 3,
   *   onExhausted: "continue",
   * });
   * ```
   */
  loop(id: string, fields: Omit<LoopSequenceFields, "id" | "kind">): SequenceHandle;
  /**
   * Declares a fan-out-over-a-collection step: runs a body once per element
   * of a list slot, binding each element to `item`. `body`/`sequence` are
   * both accepted (exactly one), like `sequence.loop`.
   * @example `sequence.forEach("review-each", { over: files, item: currentFile, body: [reviewStep] })`
   */
  forEach(id: string, fields: Omit<ForEachSequenceFields, "id" | "kind">): SequenceHandle;
  /**
   * Declares a fan-out-over-a-collection step from a closure: synthesizes
   * the per-item slot (auto-id `<step-id>-item`) and passes it to `body`,
   * which returns the inline step list to run for each element.
   * @example `sequence.forEach("review-each", files, (currentFile) => ({ body: [reviewStep] }))`
   */
  forEach<Item = unknown>(
    id: string,
    over: string | SlotHandle<readonly Item[]> | RefHandle,
    body: (
      item: SlotHandle<Item>,
    ) =>
      & { readonly body: readonly SequenceHandle[]; }
      & Omit<
        ForEachSequenceFields,
        "id" | "kind" | "over" | "item" | "sequence" | "body"
      >,
  ): SequenceHandle;
  /**
   * Declares a conditional branch: runs `success` when `check` is
   * satisfied, `failure` when it is not — at least one of the two is
   * required. `check` accepts a guard, or an unplaced `sequence.check(...)`
   * step used as a one-shot gate: the step is spliced in immediately
   * before the branch, and its declared `pass` output becomes the guard.
   * @example `sequence.branch("route", { check: condition.output.is("approved"), success: [mergeStep], failure: [reviseStep] })`
   * @example
   * ```ts
   * sequence.branch("route", {
   *   check: sequence.check("gate", { cmd: "test -f ready", output: gateReady }),
   *   success: [mergeStep],
   * });
   * ```
   */
  branch(id: string, fields: Omit<GateBranchFields, "id" | "kind">): SequenceHandle;
  /**
   * Declares a conditional branch: runs `then` when `if` is satisfied,
   * `otherwise` when it is not.
   * @deprecated Use {@link SequenceFunction.branch} — `sequence.branch(id, { check, success, failure })`.
   * @example `sequence.when("route", { if: condition.output.is("approved"), then: [mergeStep], otherwise: [reviseStep] })`
   */
  when(id: string, fields: Omit<BranchSequenceFields, "id" | "kind">): SequenceHandle;
  /**
   * Declares a parallel panel of independent named-sequence branches.
   * `max-branches` is derived from the branch count.
   * @example
   * ```ts
   * sequence.parallel("fan-out", [
   *   sequence.linear("review-branch", [reviewStep]),
   *   sequence.linear("apply-fixes-branch", [applyFixesStep]),
   * ]);
   * ```
   */
  parallel(id: string, branches: readonly SequenceRefValue[]): SequenceHandle;
  /**
   * Declares a parallel panel with a typed barrier join policy, per-branch
   * failure policy, and/or panel-level `onFailure` recovery route (P264).
   * @example
   * ```ts
   * sequence.parallel(
   *   "fan-out",
   *   [sequence.linear("review-branch", [reviewStep]), sequence.linear("apply-fixes-branch", [applyFixesStep])],
   *   { join: { policy: "collect-in-order" } },
   * );
   * ```
   */
  parallel(id: string, branches: readonly SequenceRefValue[], options: ParallelOptions): SequenceHandle;
  /** `sequence.loop(..., { onExhausted })` spellings for the two exhaustion policies, evaluating to `"continue"` and `"abort"` respectively. */
  readonly flow: {
    readonly Continue: "continue";
    readonly Abort: "abort";
  };
}

function sequencePrompt<Input, Output = unknown>(
  fields: Omit<InputPromptSequenceFields, "input" | "output"> & {
    readonly input: PromptTemplate<Input>;
    readonly output?: SequenceOutputValue<Output> | readonly SequenceOutputValue<Output>[];
  },
): SequenceHandle<Input, Output>;
function sequencePrompt<Input, Output = unknown>(
  fields: Omit<PromptSequenceFields, "prompt" | "input" | "output"> & {
    readonly prompt: PromptTemplate<Input>;
    readonly input?: SequenceInputValue<Input> | readonly SequenceInputValue<Input>[];
    readonly output?: SequenceOutputValue<Output> | readonly SequenceOutputValue<Output>[];
  },
): SequenceHandle<Input, Output>;
function sequencePrompt<Input, Output = unknown>(
  fields: Omit<TextPromptSequenceFields, "text" | "input" | "output"> & {
    readonly text: PromptTemplate<Input>;
    readonly input?: SequenceInputValue<Input> | readonly SequenceInputValue<Input>[];
    readonly output?: SequenceOutputValue<Output> | readonly SequenceOutputValue<Output>[];
  },
): SequenceHandle<Input, Output>;
function sequencePrompt(
  id: string,
  fields: Omit<PromptSequenceFields | TextPromptSequenceFields | InputPromptSequenceFields, "id" | "kind">,
): SequenceHandle;
function sequencePrompt(
  fields: PromptSequenceFields | TextPromptSequenceFields | InputPromptSequenceFields,
): SequenceHandle;
function sequencePrompt(
  idOrFields: string | PromptSequenceFields | TextPromptSequenceFields | InputPromptSequenceFields,
  maybeFields?: Omit<PromptSequenceFields | TextPromptSequenceFields | InputPromptSequenceFields, "id" | "kind">,
): SequenceHandle {
  return sequenceOf(
    typeof idOrFields === "string"
      // `maybeFields` is only ever passed alongside a string `id` — the two-
      // overload split above encodes that pairing structurally, but the
      // implementation signature (which both overloads must fit) can't carry
      // it, so the union survives here as a single documented assertion.
      ? { ...(maybeFields as object), id: idOrFields } as SequenceFields
      : idOrFields,
  );
}

function sequenceCommand(id: string, fields: Omit<CommandSequenceFields, "id" | "kind">): SequenceHandle;
function sequenceCommand(fields: CommandSequenceFields): SequenceHandle;
function sequenceCommand(
  idOrFields: string | CommandSequenceFields,
  maybeFields?: Omit<CommandSequenceFields, "id" | "kind">,
): SequenceHandle {
  if (typeof idOrFields === "string") {
    return sequenceOf({ ...(maybeFields as object), id: idOrFields, kind: "command" } as CommandSequenceFields);
  }
  recordDiagnostic(
    "cdk-legacy-sequence-command",
    `sequence.${idOrFields.id}`,
    "object-only sequence.command(fields) is deprecated; use sequence.command(id, fields)",
  );
  return sequenceOf(idOrFields);
}

function sequenceForEach(id: string, fields: Omit<ForEachSequenceFields, "id" | "kind">): SequenceHandle;
function sequenceForEach<Item = unknown>(
  id: string,
  over: string | SlotHandle<readonly Item[]> | RefHandle,
  body: (
    item: SlotHandle<Item>,
  ) =>
    & { readonly body: readonly SequenceHandle[]; }
    & Omit<
      ForEachSequenceFields,
      "id" | "kind" | "over" | "item" | "sequence" | "body"
    >,
): SequenceHandle;
function sequenceForEach(
  id: string,
  fieldsOrOver: Omit<ForEachSequenceFields, "id" | "kind"> | string | SlotHandle | RefHandle,
  body?: (item: SlotHandle) =>
    & { readonly body: readonly SequenceHandle[]; }
    & Omit<
      ForEachSequenceFields,
      "id" | "kind" | "over" | "item" | "sequence" | "body"
    >,
): SequenceHandle {
  if (body === undefined) {
    const fields = fieldsOrOver as Omit<ForEachSequenceFields, "id" | "kind">;
    return sequenceOf({ ...fields, id, kind: "for-each" } as ForEachSequenceFields);
  }
  const over = fieldsOrOver as string | SlotHandle | RefHandle;
  const item = slot.any({ id: `${id}-item`, description: `Per-item value for the ${id} loop.` });
  const closureFields = body(item);
  return sequenceOf({ ...closureFields, id, kind: "for-each", over, item } as ForEachSequenceFields);
}

function sequenceBranch(id: string, fields: Omit<GateBranchFields, "id" | "kind">): SequenceHandle {
  const { check, success, failure, ...rest } = fields;
  if (success === undefined && failure === undefined) {
    throw new Error(`procedure.sequence ${id}: branch requires success, failure, or both`);
  }
  const { guard, gateStep } = lowerCheckGuard(check, `sequence.${id}.check`);
  const branchFields = {
    ...rest,
    id,
    kind: "branch" as const,
    if: guard,
    // oxlint-disable-next-line unicorn/no-thenable -- BranchSequenceFields.then is a step ref, not a promise.
    ...(success === undefined ? {} : { then: success }),
    ...(failure === undefined ? {} : { otherwise: failure }),
  };
  const branchItem = sequenceOf(branchFields as unknown as BranchSequenceFields);
  if (gateStep !== undefined) attachMeta(branchItem, { ...metaOf(branchItem), precedingGateStep: gateStep });
  return branchItem;
}

function sequenceGate(producer: SequenceHandle, options: GateOptions): SequenceHandle {
  const producerFields = producer as unknown as {
    readonly id?: string;
    readonly output?: readonly string[];
    readonly "on-complete"?: readonly (string | { readonly signal: string; })[];
  };
  const id = producerFields.id;
  if (id === undefined) throw new Error("sequence.gate: producer must have an id");
  const proposalRef = refText(options.proposal, `sequence.${id}.gate.proposal`);
  if (!producerFields.output?.includes(proposalRef)) {
    throw new Error(`sequence.gate ${id}: producer must write ${proposalRef}`);
  }
  const signalRef = refText(options.when, `sequence.${id}.gate.when`);
  if (
    !producerFields["on-complete"]?.some((emission) =>
      typeof emission === "string" ? emission === signalRef : emission.signal === signalRef
    )
  ) {
    throw new Error(`sequence.gate ${id}: producer must declare ${signalRef}`);
  }
  if (!Number.isInteger(options.rounds) || options.rounds <= 0) {
    throw new Error(`sequence.gate ${id}: rounds must be a positive integer`);
  }
  if (options.editStep !== undefined) {
    const editFields = options.editStep as unknown as { readonly output?: readonly string[]; };
    if (!editFields.output?.includes(proposalRef)) {
      throw new Error(`sequence.gate ${id}: editStep must replace ${proposalRef}`);
    }
  }

  const decisionSchema = schema.object(`${id}-gate-decision`, {
    action: schema.enum(["approve", "edit", "reject"] as const),
    edit: schema.optional(schema.text()),
  });
  const decision = slot({
    id: `${id}-gate-decision`,
    schema: decisionSchema,
    description: `Human decision for ${id}.`,
  });
  const settled = slot.boolean(`${id}-gate-settled`);
  const seal = sequence.project(`${id}-gate-settle`, {
    projections: [{ source: operation.literal(true), destination: settled }],
  });
  const defaultEdit = sequence.project(`${id}-gate-replace`, {
    projections: [{ source: decision, field: "edit", destination: options.proposal, operation: operation.Replace }],
  });
  const editArm = options.editStep === undefined
    ? [defaultEdit, seal] as [SequenceHandle, SequenceHandle]
    : [options.editStep, seal] as [SequenceHandle, SequenceHandle];
  const routeEdit = sequence.branch(`${id}-gate-edit`, {
    check: condition.fieldEquals(decision, "action", "edit"),
    success: editArm,
  });
  const routeDecision = sequence.branch(`${id}-gate-approve`, {
    check: condition.fieldEquals(decision, "action", "approve"),
    success: [seal] as [SequenceHandle],
    failure: [routeEdit] as [SequenceHandle],
  });
  const ask = sequence.ask(`${id}-gate-ask`, {
    prompt: input
      .prompt`Review the current proposal:\n${options.proposal}\nSubmit {action: approve|edit|reject, edit?: string}.`,
    when: options.when,
    output: decision,
  });
  return sequence.loop(`${id}-gate`, {
    sequence: sequence.linear(`${id}-gate-body`, [
      sequence.project(`${id}-gate-unseal`, {
        projections: [{ source: operation.literal(false), destination: settled }],
      }),
      producer,
      ask,
      routeDecision,
    ]),
    until: condition.equals(settled, true),
    iterations: options.rounds,
    onExhausted: "abort",
  });
}

/**
 * Declares prompt, command, and control-flow sequence steps.
 * @param fields Step fields discriminated by `kind`.
 * @example `sequence.prompt({ id: "draft", text: input.prompt`Write about ${focus}.` })`
 * @see {@link procedure}
 */
export const sequence: SequenceFunction = {
  linear: function linear<Input = unknown, Output = unknown>(
    id: string,
    items: readonly SequenceHandle<Input, Output>[],
    options: { readonly description?: string; } = {},
  ): SequenceLinearHandle<Input, Output> {
    // `sequenceLinear` itself stays non-generic (`Value = unknown`) — the
    // phantom `Input`/`Output` only exist to let a caller pass this handle
    // into a step expecting a specific shape; minting them here is the one
    // justified per-call-site cast this generic wrapper exists to localize.
    return sequenceLinear(id, items, options) as SequenceLinearHandle<Input, Output>;
  },
  prompt: sequencePrompt,
  command: sequenceCommand,
  check: (
    id: string,
    fields: Omit<CheckSequenceFields, "id" | "kind">,
  ): SequenceHandle & { readonly pass: SlotHandle<boolean>; } =>
    withHiddenField(sequenceOf({ ...fields, id, kind: "check" } as CheckSequenceFields), "pass", fields.output),
  ask: (id: string, fields: Omit<AskSequenceFields, "id" | "kind">): SequenceHandle =>
    sequenceOf({ ...fields, id, kind: "ask" } as AskSequenceFields),
  gate: sequenceGate,
  project: (id: string, fields: Omit<ProjectSequenceFields, "id" | "kind">): SequenceHandle =>
    sequenceOf({ ...fields, id, kind: "project" }),
  loop: (id: string, fields: Omit<LoopSequenceFields, "id" | "kind">): SequenceHandle => {
    const until = fields.until === undefined ? undefined : lowerCheckGuard(fields.until, `sequence.${id}.until`);
    const abortIf = fields.abortIf === undefined
      ? undefined
      : lowerCheckGuard(fields.abortIf, `sequence.${id}.abortIf`);
    const gates = [until?.gateStep, abortIf?.gateStep].filter((gate): gate is SequenceHandle => gate !== undefined);
    const item = sequenceOf({
      ...fields,
      id,
      kind: "loop",
      ...(until === undefined ? {} : { until: until.guard }),
      ...(abortIf === undefined ? {} : { abortIf: abortIf.guard }),
    } as LoopSequenceFields);
    if (gates.length > 0) {
      const uniqueGates = gates.filter((gate, index) => gates.indexOf(gate) === index);
      attachMeta(item, { ...metaOf(item), loopGateSteps: uniqueGates });
    }
    return item;
  },
  forEach: sequenceForEach,
  branch: sequenceBranch,
  when: (id: string, fields: Omit<BranchSequenceFields, "id" | "kind">): SequenceHandle => {
    recordDiagnostic(
      "cdk-legacy-sequence-when",
      `sequence.${id}`,
      "sequence.when is deprecated; use sequence.branch({ check, success, failure })",
    );
    return sequenceOf({ ...fields, id, kind: "branch" });
  },
  parallel: (
    id: string,
    branches: readonly SequenceRefValue[],
    options?: ParallelOptions,
  ): SequenceHandle =>
    sequenceOf({
      id,
      kind: "parallel",
      branches,
      ...(options?.join === undefined ? {} : { join: options.join }),
      ...(options?.branchFailure === undefined ? {} : { branchFailure: options.branchFailure }),
      ...(options?.onFailure === undefined ? {} : { onFailure: options.onFailure }),
      ...(options?.include === undefined ? {} : { include: options.include }),
    }),
  flow: { Continue: "continue", Abort: "abort" } as const,
};

/**
 * Narrows `fields` to `kind`'s specific `SequenceFields` union member.
 * `sequenceOf` switches on its own `kind` local (`fields.kind ??` an
 * inferred default), not `fields.kind` directly, so TypeScript cannot
 * alias-narrow `fields` through that local the way a plain `if (fields.kind
 * === ...)` would — this is the one documented assertion every call site
 * below relies on instead of a scattered `fields as XxxSequenceFields` at
 * each branch. `kind` is validated against `expected` by reference equality
 * only; the caller is responsible for `kind` actually being `fields`'s own
 * resolved kind (every call site here passes `sequenceOf`'s own `kind`
 * local, computed once at the top of the function).
 */
function isSequenceKind<K extends NonNullable<SequenceFields["kind"]>>(
  _fields: SequenceFields,
  kind: string,
  expected: K,
): _fields is Extract<SequenceFields, { kind?: K; }> {
  return kind === expected;
}

function sequenceOf(fields: SequenceFields): SequenceHandle {
  validateSlug(fields.id, "procedure.sequence.id");
  const rawFields = fields as {
    readonly text?: unknown;
    readonly prompt?: unknown;
    readonly if?: unknown;
    readonly then?: unknown;
    readonly otherwise?: unknown;
    readonly argvFrom?: unknown;
    readonly input?: unknown;
    readonly include?: unknown;
  };
  if (rawFields.prompt !== undefined) {
    recordDiagnostic(
      "cdk-legacy-sequence-prompt",
      `sequence.${fields.id}.prompt`,
      "sequence prompt: is deprecated; use the text: field",
    );
  }
  if (rawFields.text !== undefined && rawFields.prompt !== undefined) {
    throw new Error(`procedure.sequence ${fields.id}: expected either text or prompt, not both`);
  }
  const promptInputTemplate = isPromptTemplateValue(rawFields.input) ? (rawFields.input as PromptTemplate) : undefined;
  if (promptInputTemplate !== undefined && (fields.text !== undefined || fields.prompt !== undefined)) {
    throw new Error(`procedure.sequence ${fields.id}: expected only one of input, text, or prompt`);
  }
  const prompt = fields.text ?? fields.prompt ?? promptInputTemplate;
  const commandTemplate = isCommandTemplateValue(rawFields.input) ? rawFields.input : undefined;
  const command = fields.cmd !== undefined || fields.argv !== undefined || rawFields.argvFrom !== undefined
    || commandTemplate !== undefined;
  const commandArgvFields = commandTemplate === undefined ? fields.argv : commandTemplate.argv;
  // Legacy compatibility (the landed P447 surface): a `cmd`/`argv`/`argvFrom`
  // command or check step may still declare non-interpolated dependencies —
  // including `input.optional(...)` markers — through `input:`. Trait sources
  // are evaluated, never type-checked, so silently ignoring the field would
  // drop declared refs from the step's input evidence (the exact regression
  // the optional-input proof pins). Legacy entries merge BEFORE `include:`
  // entries; with no `include:`, behavior is byte-identical to pre-include
  // emission.
  const legacyCommandDeps = commandTemplate === undefined ? fields.input : undefined;
  const commandDeps = mergeCommandDeps(legacyCommandDeps, rawFields.include as SequenceFields["input"]);
  const kind = fields.kind ?? (command ? "command" : prompt === undefined ? undefined : "prompt");
  if (kind === undefined) {
    throw new Error(
      `procedure.sequence ${fields.id}: expected prompt, ask, command, check, project, sequence, branch, loop, for-each, or parallel kind`,
    );
  }
  if (
    kind !== "branch"
    && (rawFields.if !== undefined || rawFields.then !== undefined || rawFields.otherwise !== undefined)
  ) {
    throw new Error(`procedure.sequence ${fields.id}: if, then, and otherwise are valid only on branch items`);
  }
  if (
    (kind === "prompt" || kind === "ask") !== (prompt !== undefined)
    || ((kind === "command" || kind === "check") !== command)
  ) {
    throw new Error(`procedure.sequence ${fields.id}: kind does not match prompt/command fields`);
  }
  if (
    (kind === "project" || kind === "sequence" || kind === "branch" || kind === "loop" || kind === "for-each"
      || kind === "parallel")
    && (prompt !== undefined || command)
  ) {
    throw new Error(`procedure.sequence ${fields.id}: control items must not declare prompt or command fields`);
  }
  if (isSequenceKind(fields, kind, "loop")) {
    const loop = fields;
    const hasBound = loop.iterations !== undefined || loop.maxIterations !== undefined;
    // Absent bound is a deliberate unbounded loop (0093), not an omission —
    // it must pair with an exit guard, and it makes onExhausted meaningless.
    if (!hasBound && loop.until === undefined && loop.abortIf === undefined) {
      throw new Error(`procedure.sequence ${fields.id}: unbounded loop requires until or abortIf`);
    }
    if (!hasBound && loop.onExhausted !== undefined) {
      throw new Error(
        `procedure.sequence ${fields.id}: onExhausted requires iterations or maxIterations — an unbounded loop cannot exhaust`,
      );
    }
    if (loop.iterations !== undefined && loop.maxIterations !== undefined && loop.iterations !== loop.maxIterations) {
      throw new Error(`procedure.sequence ${fields.id}: iterations and maxIterations differ`);
    }
    if (
      loop.iterations !== undefined && typeof loop.iterations !== "number" && loop.maxIterations !== undefined
    ) {
      throw new Error(`procedure.sequence ${fields.id}: dynamic iterations and maxIterations are mutually exclusive`);
    }
  }
  if (isSequenceKind(fields, kind, "for-each")) {
    const each = fields;
    if (each.limit !== undefined && each.maxItems !== undefined && each.limit !== each.maxItems) {
      throw new Error(`procedure.sequence ${fields.id}: limit and maxItems differ`);
    }
  }
  if (isSequenceKind(fields, kind, "loop") || isSequenceKind(fields, kind, "for-each")) {
    const control = fields as LoopSequenceFields | ForEachSequenceFields;
    if ((control.sequence !== undefined) === (control.body !== undefined)) {
      throw new Error(`procedure.sequence ${fields.id}: expected exactly one of sequence or body`);
    }
  }
  const project = isSequenceKind(fields, kind, "project") ? fields : undefined;
  const projections = project === undefined ? undefined : normalizeProjectProjections(project);
  const agentRef = fields.agent === undefined ? undefined : refText(fields.agent, `sequence.${fields.id}.agent`);
  const instructionOutputRender = attachInstructionOutputs(fields.id, kind, fields.output);
  const promptWithInstructionOutputs = instructionOutputRender === undefined
    ? prompt
    : mergePromptInstructionOutputs(prompt, instructionOutputRender);
  const outputTemplateRender = attachOutputTemplates(fields.id, kind, agentRef, fields.output);
  const promptWithOutputTemplates = outputTemplateRender === undefined
    ? promptWithInstructionOutputs
    // Only the prose merges into the prompt: an output-template's refs are
    // the step's own WRITE contract, not a READ the prompt interpolates, so
    // they must never join `promptInput`'s inferred input list (P105).
    : mergePromptInstructionOutputs(promptWithInstructionOutputs, {
      ...outputTemplateRender,
      refs: [],
      optionalRefs: [],
    });
  const output = projections === undefined ? outputList(fields.output) : projections.map((entry) => entry.output);
  const outputRefs = projections === undefined
    ? outputRefList(fields.output)
    : projections.map((entry) => entry.destination);
  const promptDeps = kind === "prompt" || kind === "ask"
    ? mergeCommandDeps(
      promptInputTemplate === undefined ? fields.input : undefined,
      rawFields.include as SequenceFields["input"],
    )
    : undefined;
  const input = kind === "prompt" || kind === "ask"
    ? promptInput(promptDeps, promptWithOutputTemplates)
    : isSequenceKind(fields, kind, "command")
    ? commandInput(
      commandDeps,
      commandArgvFields,
      fields.argvFrom,
      fields.executableDigestFrom,
    )
    : kind === "check"
    ? commandInput(commandDeps, commandArgvFields, undefined, undefined)
    : isSequenceKind(fields, kind, "loop")
    ? loopInput(mergeCommandDeps(fields.input, rawFields.include as SequenceFields["input"]), fields.iterations)
    : kind === "project"
    ? (() => {
      const slotSources = projections!
        .map((entry) => entry.source)
        .filter((source): source is string => typeof source === "string");
      return slotSources.length === 0 ? undefined : uniqueInOrder(slotSources);
    })()
    : (() => {
      const list = normalizeSequenceInputList(
        mergeCommandDeps(fields.input, rawFields.include as SequenceFields["input"]),
      );
      return list.length === 0 ? undefined : list;
    })();
  const implicitPrompt = kind === "prompt" || kind === "ask"
    ? promptDeclaration(
      fields,
      promptWithOutputTemplates,
      input?.filter((item) => !isOptionalInputItem(item)).map(inputItemRefText),
      outputRefs.filter((ref) => ref.startsWith("slot:")),
    )
    : undefined;
  const branch = isSequenceKind(fields, kind, "branch") ? fields : undefined;
  const thenArm = branch === undefined ? undefined : normalizeBranchArm(fields.id, "then", branch.then);
  const otherwiseArm = branch?.otherwise === undefined
    ? undefined
    : normalizeBranchArm(fields.id, "otherwise", branch.otherwise);
  const inlineThen = branch !== undefined && Array.isArray(branch.then) ? branch.then : undefined;
  const inlineOtherwise = branch?.otherwise !== undefined && Array.isArray(branch.otherwise)
    ? branch.otherwise
    : undefined;
  const inlineBranchArms = inlineThen === undefined && inlineOtherwise === undefined
    ? undefined
    : {
      ...(inlineThen === undefined ? {} : { trueArm: inlineThen }),
      ...(inlineOtherwise === undefined ? {} : { falseArm: inlineOtherwise }),
    };
  const inlineBody = (isSequenceKind(fields, kind, "loop") || isSequenceKind(fields, kind, "for-each"))
    ? (fields as LoopSequenceFields | ForEachSequenceFields).body
    : undefined;
  // Declared as `Mutable<CanonicalSequenceItem>` (P481 blocker
  // sequence-canonical-assembly-untyped): every key written below is checked
  // against the emitted model's key set, so a misspelled key (e.g.
  // `max-iteration`) is a compile error, not a silent pass-through into a
  // `deny_unknown_fields` decode. `input`/`output`/`on-complete`/`on-failure`/
  // `join`/`branch-failure` are now assigned straight from precisely typed
  // producers (P485 §3.3) with no per-assignment cast; `normalizeGuard`/
  // `guardForOutput` (guard typing, P483) and `NormalizedInputItem.when` are
  // likewise precisely typed already.
  const canonical: Mutable<CanonicalSequenceItem> = {
    id: fields.id,
    title: fields.title ?? titleFromId(fields.id),
    kind: kind === "prompt" || kind === "command" ? undefined : kind,
    agent: agentRef,
    input,
    output,
    format: fields.format === undefined || typeof fields.format === "string" ? fields.format : [...fields.format],
    "on-complete": onCompleteRules(fields.onComplete, outputRefs),
  };
  if (kind !== "branch" && kind !== "ask" && kind !== "project" && kind !== "parallel") {
    const singleStepWhen = (fields as { readonly when?: GuardValue; }).when;
    if (singleStepWhen !== undefined) canonical.when = normalizeGuard(singleStepWhen);
  }
  if (kind === "prompt" || kind === "ask") canonical.prompt = promptRef(promptWithOutputTemplates, fields.id);
  if (kind === "ask") {
    canonical.when = refText((fields as AskSequenceFields).when, `sequence.${fields.id}.when`);
  }
  if (kind === "command" || kind === "check") {
    const commandFields = isSequenceKind(fields, kind, "command") ? fields : undefined;
    canonical.command = {
      argv: commandFields?.argvFrom !== undefined
        ? undefined
        : fields.cmd === undefined
        ? validateArgv(
          (commandArgvFields ?? []).map((item, index) =>
            typeof item === "string" ? item : `{${refText(item, `sequence.${fields.id}.argv[${index}]`)}}`
          ),
          `sequence.${fields.id}.argv`,
        )
        : commandArgv(fields.cmd, `sequence.${fields.id}.cmd`),
      "argv-from": commandFields?.argvFrom === undefined
        ? undefined
        : refText(commandFields.argvFrom, `sequence.${fields.id}.argvFrom`),
      "executable-digest-from": commandFields?.executableDigestFrom === undefined
        ? undefined
        : refText(commandFields.executableDigestFrom, `sequence.${fields.id}.executableDigestFrom`),
      "timeout-ms": fields.timeoutMs ?? fields.timeout,
      "idle-timeout-ms": fields.idleTimeoutMs,
      "success-exit-code": fields.successExitCode === undefined
        ? undefined
        : typeof fields.successExitCode === "number"
        ? [fields.successExitCode]
        : [...fields.successExitCode],
    };
  }
  if (kind === "project") {
    canonical.projection = projections!.map((entry) =>
      compactAs<CanonicalProjection>({
        source: entry.source,
        field: entry.field,
        destination: entry.destination,
        operation: entry.operation === "replace" ? undefined : entry.operation,
      })
    );
  }
  if (kind === "sequence" || kind === "loop" || kind === "for-each") {
    canonical.sequence = fields.sequence === undefined
      ? undefined
      : refText(fields.sequence, `sequence.${fields.id}.sequence`);
  }
  if (kind === "branch") {
    canonical.sequence = thenArm === undefined
      ? undefined
      : refText(thenArm, `sequence.${fields.id}.then`);
    canonical.when = normalizeGuard(branch!.if);
    canonical.otherwise = otherwiseArm === undefined
      ? undefined
      : refText(otherwiseArm, `sequence.${fields.id}.otherwise`);
  }
  if ((kind === "prompt" || kind === "command") && fields.onFailure !== undefined) {
    canonical["on-failure"] = normalizeFailureTarget(
      fields.onFailure,
      `sequence.${fields.id}.onFailure`,
    );
  }
  if (isSequenceKind(fields, kind, "loop")) {
    const loop = fields;
    canonical.until = guardForOutput(loop.until as GuardValue | undefined, outputRefs);
    canonical["abort-if"] = guardForOutput(loop.abortIf as GuardValue | undefined, outputRefs);
    const iterations = loop.iterations ?? loop.maxIterations;
    canonical["max-iterations"] = typeof iterations === "number" ? iterations : undefined;
    canonical["max-iterations-from"] = iterations === undefined || typeof iterations === "number"
      ? undefined
      : refText(iterations, `sequence.${fields.id}.iterations`);
    canonical["on-exhausted"] = normalizeExhaustionTarget(loop.onExhausted, `sequence.${fields.id}.onExhausted`);
    canonical["on-abort"] = loop.onAbort === undefined
      ? undefined
      : normalizeExhaustionTarget(loop.onAbort, `sequence.${fields.id}.onAbort`);
  }
  if (isSequenceKind(fields, kind, "for-each")) {
    const each = fields;
    canonical.over = refText(each.over, `sequence.${fields.id}.over`);
    canonical.item = refText(each.item, `sequence.${fields.id}.item`);
    canonical["max-items"] = each.limit ?? each.maxItems;
    canonical["on-failure"] = each.onFailure === undefined
      ? undefined
      : normalizeFailureTarget(
        each.onFailure,
        `sequence.${fields.id}.onFailure`,
      );
    canonical.concurrent = each.concurrent === true ? true : undefined;
  }
  if (isSequenceKind(fields, kind, "parallel")) {
    const parallel = fields;
    const branchRefs = parallel.branches.map((branch, index) =>
      refText(branch, `sequence.${fields.id}.branches[${index}]`)
    );
    if (branchRefs.length === 0) {
      throw new Error(`procedure.sequence ${fields.id}: parallel requires at least one branch`);
    }
    canonical.branches = branchRefs;
    canonical["max-branches"] = branchRefs.length;
    canonical.join = normalizeParallelJoin(parallel.join, fields.id);
    canonical["branch-failure"] = normalizeBranchFailure(
      parallel.branchFailure,
      fields.id,
    );
    canonical["on-failure"] = parallel.onFailure === undefined
      ? undefined
      : normalizeFailureTarget(
        parallel.onFailure,
        `sequence.${fields.id}.onFailure`,
      );
  }
  // Control-flow refs declare too: a signal reached only via onFailure/onComplete
  // (or slots/guards via until/abortIf/over/item) must still land in the trait root.
  // `onFailure` is omitted from the loop half deliberately: loops declare it as
  // `never`, and intersecting that with for-each's real `FailureTargetValue`
  // collapses the field to `never` — which reads as "no loop has one" but
  // actually makes the shared collection site below untypable.
  const controlFields = fields as unknown as
    & Omit<Partial<LoopSequenceFields>, "onFailure">
    & Partial<ForEachSequenceFields>;
  // Same declaration-source-collection reasoning as `controlFields` above:
  // this walk reads every step-kind's variant fields off the one `fields`
  // value regardless of which kind this call actually is, so each kind's
  // fields are optionally narrowed here rather than through `SequenceFields`'
  // real discriminated union (which would make the absent-kind fields
  // statically `never`, not merely `undefined`).
  const branchFields = fields as Partial<BranchSequenceFields>;
  const parallelFields = fields as Partial<ParallelSequenceFields>;
  const parallelJoin =
    parallelFields.join?.policy === "reduce-merge" || parallelFields.join?.policy === "quorum-verdict"
      ? parallelFields.join
      : undefined;
  const declarationSources = [
    fields.agent,
    fields.prompt,
    fields.text,
    fields.argv,
    (fields as { readonly when?: GuardValue; }).when,
    // Same reasoning as `branchFields`/`parallelFields` above, inlined here
    // since each is read once.
    (fields as Partial<CommandSequenceFields>).argvFrom,
    (fields as Partial<CommandSequenceFields>).executableDigestFrom,
    fields.input,
    (fields as Partial<CommandSequenceFields | CheckSequenceFields | PromptSequenceFields | TextPromptSequenceFields>)
      .include,
    fields.output,
    fields.onComplete,
    fields.sequence,
    controlFields.iterations,
    controlFields.until,
    controlFields.abortIf,
    typeof controlFields.onFailure === "object" && controlFields.onFailure !== null && "step" in controlFields.onFailure
      ? controlFields.onFailure.signal
      : controlFields.onFailure,
    controlFields.onExhausted,
    controlFields.onAbort,
    controlFields.over,
    controlFields.item,
    branchFields.if,
    thenArm,
    otherwiseArm,
    ...(parallelFields.branches ?? []),
    parallelJoin?.destination,
    parallelJoin?.source,
    parallelFields.join?.policy === "quorum-verdict" ? parallelFields.join.guard : undefined,
    ...(parallelFields.branchFailure ?? []).map((entry) => entry.branch),
    ...(project?.projections ?? []).flatMap((entry) => [entry.source, entry.destination]),
  ];
  return withMeta(compactAs<CanonicalSequenceItem>(canonical), {
    kind: "sequence-step",
    declarations: implicitPrompt === undefined
      ? collectMany(declarationSources)
      : mergeDeclarations(collectMany(declarationSources), implicitPrompt),
    ...(inlineBranchArms === undefined ? {} : { inlineBranchArms }),
    ...(inlineBody === undefined ? {} : { inlineBody }),
  });
}
function sequenceLinear(
  id: string,
  items: readonly SequenceHandle[],
  options: {
    readonly description?: string;
    readonly generatedBranchArm?: { readonly baseId: string; };
  },
): SequenceLinearHandle {
  validateSlug(id, "sequence.id");
  const scopedItems = items.flatMap((item) => materializeSequenceItem(item, { kind: "sequence", id }));
  validateNoDuplicateTitles(scopedItems, `sequence:${id}`);
  const sequenceItems = scopedItems.map(normalizeMaterializedSequenceItem);
  const declaration = compact({ id, description: options.description, sequence: sequenceItems });
  return withDeclaration("sequence", `sequence:${id}`, declaration, {}, {
    declarations: collectMany(scopedItems),
    sourceMap: sourceMapForSequenceItems(id, scopedItems, sequenceItems),
    ...(options.generatedBranchArm === undefined ? {} : { generatedBranchArm: options.generatedBranchArm }),
  });
}
function normalizeBranchArm(
  branchId: string,
  arm: "then" | "otherwise",
  value: BranchArmValue,
): SequenceRefValue | undefined {
  if (Array.isArray(value) && value.length === 0) {
    throw new Error(`procedure.sequence ${branchId}.${arm}: branch arm must contain at least one step`);
  }
  return Array.isArray(value) ? undefined : value as SequenceRefValue;
}

/**
 * Resolves one authored sequence item into the flat item(s) it emits:
 * usually itself unchanged, but a `sequence.branch(..., { check: gateStep })`
 * expands to `[gateStep, branchItem]` (CDK grammar v2 rule 6a), and a
 * `loop`/`forEach` with an inline `body:`/inline branch arms gets its nested
 * sub-sequence(s) materialized first. Called once per item by both
 * `sequenceLinear` and `procedure`.
 */
export function materializeSequenceItem(item: SequenceHandle, scope: SequenceScope): readonly SequenceHandle[] {
  const gateStep = metaOf(item)?.precedingGateStep;
  const materialized = materializeInlineSequenceItem(item, scope);
  return gateStep === undefined ? [materialized] : [gateStep, materialized];
}

function materializeInlineSequenceItem(item: SequenceHandle, scope: SequenceScope): SequenceHandle {
  const meta = metaOf(item);
  const inline = meta?.inlineBranchArms;
  const inlineBody = meta?.inlineBody;
  const loopGates = meta?.loopGateSteps;
  if (inline === undefined && inlineBody === undefined && loopGates === undefined) return item;
  const stepId = typeof item.id === "string" ? item.id : "";
  const thenId = generatedBranchArmId(scope, stepId, "then");
  const otherwiseId = generatedBranchArmId(scope, stepId, "otherwise");
  const thenArm = inline?.trueArm === undefined
    ? undefined
    : sequenceLinear(
      thenId,
      inline.trueArm,
      { generatedBranchArm: { baseId: thenId } },
    );
  const otherwiseArm = inline?.falseArm === undefined
    ? undefined
    : sequenceLinear(
      otherwiseId,
      inline.falseArm,
      { generatedBranchArm: { baseId: otherwiseId } },
    );
  const bodyId = `${stepId}-body`;
  // A reusable body remains immutable: checks used by this loop run from a
  // deterministic wrapper invocation, not by mutating the shared declaration.
  const bodyArm = inlineBody !== undefined
    ? sequenceLinear(bodyId, [...inlineBody, ...(loopGates ?? [])], { generatedBranchArm: { baseId: bodyId } })
    : loopGates === undefined || loopGates.length === 0 || item.sequence === undefined
    ? undefined
    : sequenceLinear(
      generatedLoopGateBodyId(scope, stepId),
      [
        sequenceOf({
          id: generatedLoopGateInvocationId(stepId),
          kind: "sequence",
          sequence: refText(item.sequence, `sequence.${stepId}.sequence`),
        }),
        ...loopGates,
      ],
      { generatedBranchArm: { baseId: generatedLoopGateBodyId(scope, stepId) } },
    );
  const sequenceArm = thenArm ?? bodyArm;
  const materialized = compact({
    ...item,
    sequence: sequenceArm === undefined ? item.sequence : refText(sequenceArm, `sequence.${stepId}.sequence`),
    otherwise: otherwiseArm === undefined
      ? item.otherwise
      : refText(otherwiseArm, `sequence.${stepId}.otherwise`),
  });
  const {
    inlineBranchArms: _inlineBranchArms,
    inlineBody: _inlineBody,
    loopGateSteps: _loopGateSteps,
    ...materializedMeta
  } = meta ?? {};
  return withMeta(materialized, {
    ...materializedMeta,
    kind: "sequence-step",
    declarations: collectMany([item, thenArm, otherwiseArm, bodyArm]),
    generatedBranchArmBindings: compact({
      sequence: sequenceArm,
      otherwise: otherwiseArm,
    }) as Partial<Record<"sequence" | "otherwise", JsonObject>>,
  }) as SequenceHandle;
}

export function normalizeMaterializedSequenceItem(item: SequenceHandle): JsonObject {
  const normalized = normalizeValue(item) as JsonObject;
  const bindings = metaOf(item)?.generatedBranchArmBindings;
  return bindings === undefined
    ? normalized
    : withMeta(normalized, { generatedBranchArmBindings: bindings });
}

function generatedBranchArmId(
  scope: SequenceScope,
  branchId: string,
  arm: "then" | "otherwise",
): string {
  const encodedBranch = `${branchId.length}-${branchId}`;
  const encodedScope = scope.kind === "procedure" ? "p" : `s-${scope.id.length}-${scope.id}`;
  return `ctx-branch-arm-${encodedScope}-${encodedBranch}-${arm}`;
}

function generatedLoopGateBodyId(scope: SequenceScope, loopId: string): string {
  const encodedScope = scope.kind === "procedure" ? "p" : `s-${scope.id.length}-${scope.id}`;
  return `ctx-loop-gate-${encodedScope}-${loopId.length}-${loopId}`;
}

function generatedLoopGateInvocationId(loopId: string): string {
  return `ctx-loop-gate-invoke-${loopId.length}-${loopId}`;
}
/** True for an `${slot.optional()}` output marker (P105): `{ slot, optional: true }`, mirroring `OptionalSlotInputValue` on the input side. */
function isOptionalOutputItem(item: unknown): item is OptionalSlotRead {
  return item !== null && typeof item === "object" && !Array.isArray(item) && "slot" in item
    && (item as { readonly optional?: unknown; }).optional === true;
}
function outputText(value: SequenceOutputValue, path: string): string {
  if (isOptionalOutputItem(value)) return refText(value.slot, path);
  const declaration = outputSinkDeclaration(value);
  return declaration !== undefined ? declaration.slot : refText(value, path);
}
/**
 * Expands an `output.prompt` output-template item into its interpolated
 * slots (each as an ordinary ref string or `{ slot, optional: true }`
 * marker, in interpolation order) — every other item passes through
 * unchanged. Shared by `outputList`/`outputRefList` so an output-template
 * lowers identically wherever a step's `output:` list is read.
 */
function flattenOutputItems(
  value: SequenceOutputValue | readonly SequenceOutputValue[] | undefined,
): readonly SequenceOutputValue[] {
  if (value === undefined) return [];
  const items = Array.isArray(value) ? value : [value];
  return items.flatMap((item) => {
    const content = outputTemplateContent(item);
    if (content === undefined) return [item];
    const optionalRefs = new Set(content.optionalRefs ?? []);
    return content.refs.map((ref) =>
      (optionalRefs.has(ref) ? { slot: ref, optional: true } : ref) as unknown as SequenceOutputValue
    );
  });
}
function outputList(
  value: SequenceOutputValue | readonly SequenceOutputValue[] | undefined,
): CanonicalOutputSink[] | undefined {
  if (value === undefined) return undefined;
  return flattenOutputItems(value).map((item, index) => {
    const path = `sequence.output[${index}]`;
    if (isOptionalOutputItem(item)) return { slot: refText(item.slot, path), optional: true };
    return metaOf(item)?.kind === "output-sink"
      // A `SlotHandle | operation.over(...)` output-sink item normalizes to
      // the canonical `{ slot, operation }` shape by construction (see
      // `slot.ts`'s `operation.over`) — `normalizeValue` itself walks
      // arbitrary JSON, so its return stays `JsonValue` at the boundary.
      ? normalizeValue(item) as CanonicalOutputSink
      : outputText(item, path);
  });
}
function outputRefList(value: SequenceOutputValue | readonly SequenceOutputValue[] | undefined): string[] {
  return flattenOutputItems(value).map((item, index) => outputText(item, `sequence.output[${index}]`));
}
function promptRef(value: unknown, id: string): string {
  const meta = metaOf(value);
  if (meta?.kind === "template") return `prompt:${id}`;
  if (meta?.ref !== undefined) return meta.ref;
  if (typeof value === "string") return value;
  throw new Error(`procedure.sequence ${id}: unsupported prompt value`);
}
/** A normalized `procedure.sequence[].input` entry: a plain ref, a
 * guard-conditioned resource ref lowered to canonical `{ ref, when }`
 * (P290), or an optional-slot marker lowered to canonical
 * `{ slot, optional: true }` (P447). */
type NormalizedInputItem =
  | string
  | { readonly ref: string; readonly when: CanonicalGuardExpr; }
  | { readonly slot: string; readonly optional: true; };

function inputItemRefText(item: NormalizedInputItem): string {
  if (typeof item === "string") return item;
  return "ref" in item ? item.ref : item.slot;
}

function isOptionalInputItem(item: NormalizedInputItem): boolean {
  return typeof item === "object" && "optional" in item;
}

/** Dedupe a mixed list of plain refs, guarded-resource, and optional-slot entries by ref text, first occurrence wins. */
function uniqueInputsInOrder(items: readonly NormalizedInputItem[]): NormalizedInputItem[] {
  const seen = new Set<string>();
  const result: NormalizedInputItem[] = [];
  for (const item of items) {
    const ref = inputItemRefText(item);
    if (seen.has(ref)) continue;
    seen.add(ref);
    result.push(item);
  }
  return result;
}

function normalizeSequenceInputItem(item: unknown, path: string): NormalizedInputItem {
  if (item !== null && typeof item === "object" && !Array.isArray(item) && "resource" in item && "when" in item) {
    const conditional = item as ConditionalResourceInputValue;
    const ref = refText(conditional.resource, `${path}.resource`);
    if (!ref.startsWith("resource:")) throw new Error(`${path}.resource: expected resource:* ref`);
    const when = normalizeGuard(conditional.when);
    if (when === undefined) throw new Error(`${path}.when: guard is required`);
    return { ref, when };
  }
  if (item !== null && typeof item === "object" && !Array.isArray(item) && "slot" in item && "optional" in item) {
    const optionalSlotItem = item as OptionalSlotInputValue;
    if (optionalSlotItem.optional !== true) {
      throw new Error(`${path}.optional: input.optional() marker must have optional = true`);
    }
    const ref = refText(optionalSlotItem.slot, `${path}.slot`);
    if (!ref.startsWith("slot:")) throw new Error(`${path}.slot: expected slot:* ref`);
    return { slot: ref, optional: true };
  }
  return refText(item, path);
}

function normalizeSequenceInputList(value: SequenceFields["input"]): NormalizedInputItem[] {
  if (value === undefined) return [];
  const values = Array.isArray(value) ? value : [value];
  return values.map((item, index) => normalizeSequenceInputItem(item, `sequence input item ${index}`));
}

function promptInput(value: SequenceFields["input"], prompt: unknown): NormalizedInputItem[] | undefined {
  const explicit = normalizeSequenceInputList(value);
  const explicitRefs = explicit.map(inputItemRefText);
  const promptMeta = metaOf(prompt);
  const optionalRefs = new Set(promptMeta?.optionalRefs ?? []);
  const inferred = (promptMeta?.refs ?? []).filter((ref) =>
    ref.startsWith("port:") || ref.startsWith("slot:") || ref.startsWith("resource:")
  );
  for (const ref of explicitRefs) {
    if (inferred.includes(ref)) {
      recordDiagnostic(
        "cdk-duplicate-input",
        "sequence.input",
        `explicit input ${ref} duplicates a reference interpolated into the prompt`,
      );
    }
  }
  // An interpolation written `slot.optional()` becomes the optional input
  // form, so the producer-before-consumer check exempts it and a loop-carried
  // read (including a step reading the slot it also writes) is expressible.
  const inferredItems: NormalizedInputItem[] = inferred.map((ref) =>
    optionalRefs.has(ref) ? { slot: ref, optional: true } : ref
  );
  const merged = inferred.every((ref) => explicitRefs.includes(ref))
    ? uniqueInputsInOrder(explicit)
    : uniqueInputsInOrder([...inferredItems, ...explicit]);
  return merged.length === 0 ? undefined : merged;
}
function isCommandTemplateValue(value: unknown): value is CommandTemplateValue {
  return typeof value === "object" && value !== null && (value as { kind?: unknown; }).kind === "cdk-command-template";
}
/** True for an `input.prompt` tagged-template value — the prompt-step `input:` carrier. */
function isPromptTemplateValue(value: unknown): value is PromptTemplate {
  return metaOf(value)?.kind === "template";
}
/**
 * Auto-attaches every `output.text`/`output.of(...)` instruction-output
 * listed in a step's `output:` to a slot (id: the step id, then
 * `<step-id>-2`... for later ones on the same step), erroring if the
 * auto-assigned id collides with a hand-declared slot in the same list.
 * Returns the rendered instruction/return-format text (plus any refs the
 * instruction text itself interpolated) to append onto the step's prompt
 * body, or `undefined` if the step declares no instruction-outputs.
 */
function attachInstructionOutputs(
  stepId: string,
  kind: SequenceFields["kind"],
  outputValue: SequenceOutputValue | readonly SequenceOutputValue[] | undefined,
): { readonly text: string; readonly refs: readonly string[]; readonly optionalRefs: readonly string[]; } | undefined {
  if (outputValue === undefined) return undefined;
  const items = Array.isArray(outputValue) ? outputValue : [outputValue];
  const instructionOutputs = items.filter((item) => isInstructionOutputHandle(item));
  if (instructionOutputs.length === 0) return undefined;
  if (kind !== "prompt" && kind !== "ask") {
    throw new Error(`procedure.sequence ${stepId}: output.text/output.of is valid only on prompt/ask steps`);
  }
  const usedIds = new Set(
    items
      .filter((item) => !isInstructionOutputHandle(item))
      .map((item) => outputText(item, `sequence.${stepId}.output`))
      .filter((ref) => ref.startsWith("slot:"))
      .map((ref) => ref.slice("slot:".length)),
  );
  let text = "";
  const refs: string[] = [];
  const optionalRefs: string[] = [];
  let counter = 1;
  for (const handle of instructionOutputs) {
    const autoId = counter === 1 ? stepId : `${stepId}-${counter}`;
    counter += 1;
    if (usedIds.has(autoId)) {
      throw new Error(
        `procedure.sequence ${stepId}: output.text/output.of auto-declared slot ${autoId} collides with the `
          + `hand-declared slot ${autoId} in the same output: list — rename the hand-declared slot`,
      );
    }
    usedIds.add(autoId);
    const content = instructionOutputContent(handle);
    if (content === undefined) {
      throw new Error(`procedure.sequence ${stepId}: expected an output.text/output.of value in output:`);
    }
    const slotDeclaration = compact({
      id: autoId,
      schema: content.schemaRef ?? "schema:text",
      description: `Runtime slot ${autoId}.`,
    });
    attachInstructionOutput(handle, `slot:${autoId}`, slotDeclaration);
    text += (text === "" ? "" : "\n\n")
      + (content.schemaRef === undefined
        ? OUTPUT_RENDER_V1.text(content.text)
        : OUTPUT_RENDER_V1.of(content.text, content.schemaRef));
    refs.push(...(content.refs ?? []));
    optionalRefs.push(...(content.optionalRefs ?? []));
  }
  return { text, refs: uniqueInOrder(refs), optionalRefs: uniqueInOrder(optionalRefs) };
}
/**
 * Appends an instruction-output render block onto the attaching step's
 * prompt body: the merged text/refs a template-kind `prompt`/`input`/`text`
 * value carries. A named `prompt(...)` reference (not a template) cannot
 * absorb the render — its text lives in a separately declared prompt object
 * this step does not own.
 */
function mergePromptInstructionOutputs(
  promptValue: unknown,
  render: { readonly text: string; readonly refs: readonly string[]; readonly optionalRefs: readonly string[]; },
): PromptTemplate {
  const meta = metaOf(promptValue);
  if (meta?.kind !== "template") {
    throw new Error(
      "output.text/output.of/output.prompt: the attaching step's prompt body must be an input.prompt value, "
        + "not a named prompt(...) reference",
    );
  }
  const baseText = typeof (promptValue as unknown as { readonly text?: unknown; }).text === "string"
    ? (promptValue as unknown as { readonly text: string; }).text
    : "";
  const text = `${baseText}\n\n${render.text}`;
  return withMeta({ text }, {
    kind: "template",
    refs: uniqueInOrder([...(meta.refs ?? []), ...render.refs]),
    optionalRefs: uniqueInOrder([...(meta.optionalRefs ?? []), ...render.optionalRefs]),
    declaration: { text },
    ...(meta.declarations === undefined ? {} : { declarations: meta.declarations }),
  }) as PromptTemplate;
}
/**
 * Merges every `output.prompt` output-template in a step's `output:` list
 * into the step's compiled prompt, and enforces the P105 build rules: a
 * slot claimed twice across this step's own `output:` entries (template x
 * template, or template x plain) is an error, and a slot claimed by
 * `output.prompt` on two steps for different `agent:` values is an error
 * (`claimOutputTemplateAuthorship`, meta.ts). Returns `undefined` when the
 * step declares no output-template, so its prompt/refs stay byte-identical
 * to hand-listing the same slots and hand-writing the same prose.
 */
function attachOutputTemplates(
  stepId: string,
  kind: SequenceFields["kind"],
  agentRef: string | undefined,
  outputValue: SequenceOutputValue | readonly SequenceOutputValue[] | undefined,
): { readonly text: string; readonly refs: readonly string[]; readonly optionalRefs: readonly string[]; } | undefined {
  if (outputValue === undefined) return undefined;
  const items = Array.isArray(outputValue) ? outputValue : [outputValue];
  const templates = items.filter((item) => isOutputTemplateHandle(item));
  if (templates.length === 0) return undefined;
  if (kind !== "prompt" && kind !== "ask") {
    throw new Error(`procedure.sequence ${stepId}: output.prompt is valid only on prompt/ask steps`);
  }
  const claimedBy = new Map<string, "template" | "plain">();
  for (const item of items) {
    if (isOutputTemplateHandle(item)) continue;
    claimedBy.set(outputText(item, `sequence.${stepId}.output`), "plain");
  }
  let text = "";
  const refs: string[] = [];
  const optionalRefs: string[] = [];
  for (const handle of templates) {
    const content = outputTemplateContent(handle);
    if (content === undefined) {
      throw new Error(`procedure.sequence ${stepId}: expected an output.prompt value in output:`);
    }
    const templateOptionalRefs = new Set(content.optionalRefs ?? []);
    content.refs.forEach((ref, index) => {
      if (claimedBy.has(ref)) {
        throw new Error(
          `procedure.sequence ${stepId}: output slot ${ref} is claimed more than once across this step's `
            + `output: entries`,
        );
      }
      claimedBy.set(ref, "template");
      claimOutputTemplateAuthorship(content.slots[index], agentRef, stepId);
      refs.push(ref);
      if (templateOptionalRefs.has(ref)) optionalRefs.push(ref);
    });
    text += (text === "" ? "" : "\n\n") + content.text;
  }
  return { text, refs: uniqueInOrder(refs), optionalRefs: uniqueInOrder(optionalRefs) };
}
/**
 * Merges a legacy `input:` dependency list (P447-era command/check surface)
 * with `include:` entries, legacy first. Either side absent returns the
 * other unchanged, so a legacy-only step's input evidence stays
 * byte-identical to its pre-`include` emission.
 */
function mergeCommandDeps(
  legacy: SequenceFields["input"],
  include: SequenceFields["input"],
): SequenceFields["input"] {
  if (legacy === undefined) return include;
  if (include === undefined) return legacy;
  const asList = (value: NonNullable<SequenceFields["input"]>): readonly SequenceInputValue[] =>
    Array.isArray(value) ? value : [value as SequenceInputValue];
  return [...asList(legacy), ...asList(include)];
}
function commandInput(
  value: SequenceFields["input"],
  argv: readonly ArgvItem[] | undefined,
  argvFrom: PortHandle<string[]> | undefined,
  executableDigestFrom: PortHandle<string> | SlotHandle<string> | undefined,
): NormalizedInputItem[] | undefined {
  const explicit = normalizeSequenceInputList(value);
  const explicitRefs = explicit.map(inputItemRefText);
  const inferred = (argv ?? []).flatMap((item) =>
    typeof item === "string" ? scanInterpolationRefs(item) : [metaOf(item)?.ref ?? ""]
  ).filter(
    (ref) => ref.startsWith("slot:") || ref.startsWith("port:") || ref.startsWith("resource:"),
  );
  if (argvFrom !== undefined) inferred.push(refText(argvFrom, "sequence.command.argvFrom"));
  if (executableDigestFrom !== undefined) {
    inferred.push(refText(executableDigestFrom, "sequence.command.executableDigestFrom"));
  }
  const merged = inferred.every((ref) => explicitRefs.includes(ref))
    ? uniqueInputsInOrder(explicit)
    : uniqueInputsInOrder([...inferred, ...explicit]);
  return merged.length === 0 ? undefined : merged;
}
function loopInput(
  value: SequenceFields["input"],
  iterations: number | PortHandle<number> | undefined,
): NormalizedInputItem[] | undefined {
  const explicit = normalizeSequenceInputList(value);
  if (iterations === undefined || typeof iterations === "number") {
    return explicit.length === 0 ? undefined : explicit;
  }
  return uniqueInputsInOrder([refText(iterations, "sequence.loop.iterations"), ...explicit]);
}

type ProjectWriteOperation = "replace" | "append" | "increment";

interface NormalizedProjectProjection {
  /** A local slot ref string, or the wrapped canonical literal `{ literal: <value> }` (P431). */
  readonly source: string | { readonly literal: JsonValue; };
  readonly field?: string;
  readonly destination: string;
  readonly operation: ProjectWriteOperation;
  readonly output: CanonicalOutputSink;
}

function normalizeProjectProjections(fields: ProjectSequenceFields): NormalizedProjectProjection[] {
  if (fields.projections.length === 0) {
    throw new Error(`procedure.sequence ${fields.id}: project requires at least one projection`);
  }
  return fields.projections.map((projection, index) => {
    const path = `sequence.${fields.id}.projections[${index}]`;
    const destination = outputText(projection.destination, `${path}.destination`);
    if (!destination.startsWith("slot:")) {
      throw new Error(`${path}: project destination must be a local slot handle`);
    }
    const isLiteral = isLiteralProjectionSource(projection.source);
    if (isLiteral && projection.field !== undefined) {
      throw new Error(`${path}.field: a literal project source has no field to select`);
    }
    const source: string | { readonly literal: JsonValue; } = isLiteral
      ? { literal: (projection.source as LiteralProjectionSource).literal }
      : refText(projection.source as SlotHandle, `${path}.source`);
    if (!isLiteral && !(source as string).startsWith("slot:")) {
      throw new Error(`${path}: project source must be a local slot handle or operation.literal(...)`);
    }
    if (projection.field !== undefined) validateSlug(projection.field, `${path}.field`);
    const declaration = outputSinkDeclaration(projection.destination);
    const handleOperation = declaration !== undefined
      ? normalizeProjectOperation(declaration.operation, `${path}.destination`)
      : undefined;
    const explicitOperation = projection.operation === undefined
      ? undefined
      : normalizeProjectOperation(projection.operation, `${path}.operation`);
    if (
      explicitOperation !== undefined
      && handleOperation !== undefined
      && JSON.stringify(explicitOperation) !== JSON.stringify(handleOperation)
    ) {
      throw new Error(`${path}.operation: explicit operation conflicts with destination operation handle`);
    }
    const operation = explicitOperation ?? handleOperation ?? "replace";
    return {
      source,
      ...(projection.field === undefined ? {} : { field: projection.field }),
      destination,
      operation,
      output: operation === "replace" ? destination : { slot: destination, operation },
    };
  });
}

function normalizeProjectOperation(value: JsonValue | undefined, path: string): ProjectWriteOperation | undefined {
  if (value === undefined) return undefined;
  if (value === "replace" || value === "append" || value === "increment") return value;
  throw new Error(`${path}: project operation must be replace, append, or increment`);
}
function scanInterpolationRefs(value: string): string[] {
  const refs: string[] = [];
  const pattern = /\{([^{}]+)\}/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(value)) !== null) {
    const previous = match.index > 0 ? value[match.index - 1] : "";
    if (previous !== "$" && previous !== "{" && match[1] !== undefined && REF_TOKEN_PATTERN.test(match[1])) {
      refs.push(match[1]);
    }
  }
  return refs;
}
function promptDeclaration(
  fields: SequenceFields,
  value: unknown,
  input: readonly string[] | undefined,
  output: readonly string[],
): JsonObject | undefined {
  const meta = metaOf(value);
  if (meta?.kind !== "template") return undefined;
  const normalized = normalizeValue(value) as JsonObject;
  const declaration = compact({
    id: fields.id,
    input,
    output: output.length === 0 ? undefined : output,
    description: fields.description,
    text: typeof normalized.text === "string" ? normalized.text : undefined,
    source: typeof normalized.source === "string" ? normalized.source : undefined,
  });
  return withMeta(
    declaration,
    compact({
      kind: "prompt",
      ref: `prompt:${fields.id}`,
      declaration,
      source: meta.source,
    }) as import("./meta.js").Meta,
  );
}
function mergeDeclarations(
  declarations: ReturnType<typeof collectMany>,
  prompt: JsonObject,
): ReturnType<typeof collectMany> {
  return { ...declarations, prompt: [...(declarations.prompt ?? []), prompt] };
}
function guardForOutput(value: GuardValue | undefined, outputs: readonly string[]): CanonicalGuardExpr | undefined {
  const normalized = normalizeGuard(value);
  const paths = defaultOutputPaths(value);
  if (paths.length === 0) return normalized;
  if (outputs.length !== 1) {
    throw new Error(
      "condition.output.is(value): sequence must declare exactly one output or pass the output handle explicitly",
    );
  }
  // `bindDefaultOutput` stays `JsonValue`-shaped — it's a generic structural
  // walk over arbitrary JSON (see its own rationale comment below) — so a
  // guard, itself precisely `CanonicalGuardExpr`-typed, crosses that
  // boundary and back through one cast on each side of the reduce.
  return paths.reduce<CanonicalGuardExpr | undefined>(
    (guard, path) =>
      guard === undefined
        ? undefined
        : bindDefaultOutput(guard as JsonValue, path, outputs[0] as string) as CanonicalGuardExpr,
    normalized,
  );
}
function bindDefaultOutput(value: JsonValue, path: readonly (string | number)[], output: string): JsonValue {
  if (path.length === 0) {
    return value !== null && typeof value === "object" && !Array.isArray(value)
      ? compact({ ...value, output })
      : value;
  }
  const [segment, ...rest] = path;
  if (typeof segment === "number" && Array.isArray(value) && value[segment] !== undefined) {
    return value.map((item, index) => index === segment ? bindDefaultOutput(item, rest, output) : item);
  }
  if (typeof segment === "string" && value !== null && typeof value === "object" && !Array.isArray(value)) {
    // `readonly JsonValue[]` (P481: readonly canonical arrays now flow
    // through JsonValue) isn't excluded by `!Array.isArray` the way a
    // mutable `JsonValue[]` arm was — `value` is genuinely a plain object
    // here (the two runtime checks above already guarantee it).
    const object = value as JsonObject;
    const child = object[segment];
    return child === undefined ? value : { ...object, [segment]: bindDefaultOutput(child, rest, output) };
  }
  return value;
}
function onCompleteRules(
  value: SequenceFields["onComplete"],
  outputs: readonly string[],
): CanonicalSignalEmissionRule[] | undefined {
  if (value === undefined) return undefined;
  return (Array.isArray(value) ? value : [value]).map((item, index) =>
    typeof item === "string" || metaOf(item)?.ref !== undefined
      ? signalText(item, `sequence.on-complete[${index}]`)
      : compact({
        signal: signalText(item.signal, `sequence.on-complete[${index}].signal`),
        when: guardForOutput(item.when, outputs),
        // `compact` erases back to `JsonObject`; the object literal above is
        // already exactly `CanonicalSignalEmissionRule`'s object member shape.
      }) as CanonicalSignalEmissionRule
  );
}
function signalText(value: unknown, path: string): string {
  const text = refText(value, path);
  if (!text.startsWith("signal:")) throw new Error(`${path}: expected signal:* ref`);
  return text;
}
function normalizeParallelJoin(
  value: ParallelJoinOption | undefined,
  id: string,
): CanonicalJoinPolicy | undefined {
  if (value === undefined) return undefined;
  if (value.policy === "collect-in-order") return { policy: "collect-in-order" };
  // `compact` erases back to `JsonObject`; each object literal below is
  // already exactly one `CanonicalJoinPolicy` union member's shape.
  if (value.policy === "reduce-merge") {
    return compact({
      policy: "reduce-merge",
      destination: refText(value.destination, `sequence.${id}.join.destination`),
      source: refText(value.source, `sequence.${id}.join.source`),
      operation: value.operation,
    }) as CanonicalJoinPolicy;
  }
  return compact({
    policy: "quorum-verdict",
    destination: refText(value.destination, `sequence.${id}.join.destination`),
    source: refText(value.source, `sequence.${id}.join.source`),
    guard: normalizeGuard(value.guard),
  }) as CanonicalJoinPolicy;
}
function normalizeBranchFailure(
  value: readonly ParallelBranchFailureEntry[] | undefined,
  id: string,
): CanonicalBranchFailureEntry[] | undefined {
  if (value === undefined || value.length === 0) return undefined;
  return value.map((entry, index) =>
    compactAs<CanonicalBranchFailureEntry>({
      branch: refText(entry.branch, `sequence.${id}.branch-failure[${index}].branch`),
      "on-failure": entry.onFailure,
    })
  );
}
function normalizeExhaustionTarget(
  value: ExhaustionPolicy | ExhaustionSignalValue | readonly ExhaustionSignalValue[] | undefined,
  path: string,
): CanonicalExhaustionTarget | undefined {
  if (value === undefined) return undefined;
  if (value === "continue" || value === "abort") return value;
  if (Array.isArray(value)) {
    if (value.length === 0) throw new Error(`${path}: must not be empty`);
    return value.map((entry, index) => signalText(entry, `${path}[${index}]`));
  }
  return signalText(value as ExhaustionSignalValue, path);
}
function normalizeFailureTarget(value: FailureTargetValue, path: string): CanonicalFailureTarget {
  if (typeof value === "string" || metaOf(value)?.ref !== undefined) {
    return signalText(value, path);
  }
  if (!value.step) throw new Error(`${path}.step: must name a recovery step`);
  // `value` stays a `FailureRoute | RefHandle<"signal">` union here (the
  // `RefHandle` arm's index signature makes `.step` structurally `unknown`,
  // not narrowable further by the truthiness check above) — one cast at the
  // return, not per-field.
  return compact({
    step: value.step,
    signal: value.signal === undefined ? undefined : signalText(value.signal, `${path}.signal`),
  }) as CanonicalFailureRoute;
}
function commandArgv(cmd: string, path: string): string[] {
  if (cmd.trim() !== cmd || cmd.length === 0) {
    throw new Error(`${path}: command shorthand contains shell-only syntax; use explicit argv`);
  }
  return validateArgv(tokenizeShellLiteral(cmd, path), path);
}
function validateArgv(argv: readonly string[], path: string): string[] {
  if (argv.length === 0) throw new Error(`${path}: argv must not be empty`);
  const normalized = argv.map((arg, index) => {
    if (arg.trim() !== arg || arg.length === 0) {
      throw new Error(`${path}[${index}]: argv item must be non-empty and trimmed`);
    }
    return arg;
  });
  if (normalized[0]?.includes("..")) throw new Error(`${path}: relative program path must not contain ..`);
  if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(normalized[0] ?? "")) {
    throw new Error(`${path}: environment assignment shorthand is unsupported`);
  }
  return normalized;
}
function titleFromId(id: string): string {
  return id.split("-").filter(Boolean).map((part) => part[0]?.toUpperCase() + part.slice(1)).join(" ") || id;
}

/**
 * Derives a step id from its declared title: lowercase, non-alphanumeric
 * runs collapsed to a single hyphen, validated against the shared slug
 * grammar (`validateSlug`, normalize.ts) — never re-slugified per call site.
 * `titleFromId`'s inverse; the entry point 0106's functional (title-only)
 * steps will call to mint their own id.
 */
export function idFromTitle(title: string): string {
  const id = title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  validateSlug(id, `idFromTitle(${JSON.stringify(title)})`);
  return id;
}

/**
 * Build error when two steps in the same scope (a `sequence:` array, or a
 * procedure's top-level sequence) derive the same id from their titles —
 * per 0104, uniquifying by counter would smuggle authoring-order-dependence
 * back into generated ids, so this fails loudly instead.
 */
export function validateNoDuplicateTitles(items: readonly SequenceHandle[], scopeLabel: string): void {
  const seen = new Map<string, string>();
  for (const item of items) {
    const title = typeof item.title === "string" ? item.title : undefined;
    if (title === undefined) continue;
    const id = idFromTitle(title);
    const existing = seen.get(id);
    if (existing !== undefined) {
      throw new Error(
        `${scopeLabel}: steps titled ${JSON.stringify(existing)} and ${JSON.stringify(title)} both derive id ${
          JSON.stringify(id)
        }`,
      );
    }
    seen.set(id, title);
  }
}
