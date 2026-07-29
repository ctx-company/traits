import type { PortHandle, ResourceHandle, SlotHandle } from "./handles.js";
import { tokenizeShellLiteral } from "./normalize.js";
import { refText } from "./ref.js";
import type { ArgvItem } from "./sequence.js";

/**
 * A sequence-item slot input that never gates validation, planning,
 * recovery, or runtime readiness (P447). Present through the normal
 * available-input path only once an accepted value exists; otherwise the
 * ref is omitted entirely from the frame.
 */
export type OptionalSlotInputValue<Value = unknown> = {
  readonly slot: SlotHandle<Value>;
  readonly optional: true;
};

/** A value accepted as one `input.command` tagged-template interpolation. */
export type CommandInterpolation = SlotHandle | PortHandle | ResourceHandle;

/**
 * A hermetically tokenized command template, consumed by `sequence.command`'s
 * and `sequence.check`'s `input:` field. Distinct from the raw `argv:`
 * escape hatch so a command/check step's `input:` unambiguously means "build
 * argv from this template" rather than "declare this dependency" — non-
 * interpolated dependencies for a command/check step are declared through
 * that step's `include:` field instead.
 */
export type CommandTemplateValue = {
  readonly kind: "cdk-command-template";
  readonly argv: readonly ArgvItem[];
};

export interface InputFunction {
  /**
   * Declares a sequence-item slot input as optional: the slot never blocks
   * production, recovery routing, or dry-plan readiness, and is included in
   * the frame only once an accepted value exists.
   *
   * Cannot satisfy required prompt interpolation, an explicit prompt
   * contract, or command argv interpolation — those all require a value to
   * be present unconditionally, and an absent optional slot would leave the
   * interpolation unresolved.
   * @example `sequence.prompt("produce", { agent: worker, text: prompt.text`Produce a draft.`, input: input.optional(verdict), output: draft })`
   */
  optional<Value>(slot: SlotHandle<Value>): OptionalSlotInputValue<Value>;

  /**
   * Builds a hermetically tokenized command template from a tagged template
   * literal, for `sequence.command`'s and `sequence.check`'s `input:` field.
   * Each literal segment is tokenized into argv words with shell-style
   * quoting and escaping (rejecting shell-execution syntax — no shell is
   * ever invoked), and each interpolated handle becomes exactly one complete
   * argv element rather than being spliced into a word. Lowers through the
   * same argv validation as an explicit `argv:` array. Non-interpolated
   * dependencies — including `input.optional(...)` markers — are declared
   * separately through `include:`, not by interpolating them here.
   * @example `sequence.command({ id: "commit", input: input.command`git commit -m ${message}`, include: input.optional(scope) })`
   */
  command(
    strings: TemplateStringsArray,
    ...values: readonly CommandInterpolation[]
  ): CommandTemplateValue;
}

export function optionalSlot<Value>(slot: SlotHandle<Value>): OptionalSlotInputValue<Value> {
  const ref = refText(slot, "input.optional");
  if (!ref.startsWith("slot:")) {
    throw new Error("input.optional: expected a slot handle");
  }
  return { slot, optional: true };
}

function commandTemplate(
  strings: TemplateStringsArray,
  ...values: readonly CommandInterpolation[]
): CommandTemplateValue {
  const items: ArgvItem[] = [];
  strings.forEach((segment, index) => {
    const isFirst = index === 0;
    const isLast = index === strings.length - 1;
    // A segment right after an interpolation (not first) starting with a
    // non-whitespace character means that interpolated value has no
    // whitespace *after* it in the source.
    if (!isFirst && segment.length > 0 && /^\S/.test(segment)) {
      throw new Error(
        "input.command: interpolated value must be a standalone argv token; add whitespace after it",
      );
    }
    // A segment right before an interpolation (not last) ending with a
    // non-whitespace character means that interpolated value has no
    // whitespace *before* it in the source.
    if (!isLast && segment.length > 0 && /\S$/.test(segment)) {
      throw new Error(
        "input.command: interpolated value must be a standalone argv token; add whitespace before it",
      );
    }
    items.push(...tokenizeShellLiteral(segment, "input.command"));
    if (!isLast) {
      const value = values[index];
      if (value === undefined) throw new Error(`input.command: missing interpolated value ${index}`);
      refText(value, `input.command value ${index}`);
      items.push(value);
    }
  });
  if (items.length === 0) throw new Error("input.command: argv must not be empty");
  return { kind: "cdk-command-template", argv: items };
}

/** Sequence-item input authoring markers. @see {@link InputFunction} */
export const input: InputFunction = {
  optional: optionalSlot,
  command: commandTemplate,
};
