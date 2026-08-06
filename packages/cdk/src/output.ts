import type { InstructionOutputHandle, OptionalSlotRead, OutputTemplateHandle, SlotHandle } from "./handles.js";
import {
  attachInstructionOutput,
  instructionOutputContent,
  isInstructionOutputHandle,
  metaOf,
  withMeta,
} from "./meta.js";
import { collectMany, mergeDeclarationSets } from "./normalize.js";
import type { PromptInterpolation } from "./prompt.js";
import { promptTemplate } from "./prompt.js";
import { refText } from "./ref.js";
import type { SchemaValue } from "./schema.js";

/**
 * The versioned instruction/return-format wording `output.text`/`output.of`
 * render into their attaching step's compiled prompt. Digest-covered: any
 * future wording change moves every canonical built with an
 * instruction-output, so a revision ships as `OUTPUT_RENDER_V2` alongside
 * this one, never an in-place edit.
 */
export const OUTPUT_RENDER_V1 = {
  text: (instruction: string): string => `${instruction}\n\nReturn plain text only, with no surrounding commentary.`,
  of: (instruction: string, schemaRef: string): string =>
    `${instruction}\n\nReturn JSON matching ${schemaRef}, with no surrounding commentary.`,
};

export interface OutputFunction {
  /**
   * Builds a plain-text instruction-output: usable anywhere a sequence
   * step's `output:` accepts a slot handle. Auto-declares a `schema:text`
   * slot (id: the step id, or `<step-id>-2`... for a second anonymous
   * output on the same step) and renders the instruction text plus a
   * return-format instruction into the attaching step's compiled prompt —
   * one source of truth for the prompt, the slot, and its schema.
   * @example `sequence.prompt("summarize", { ..., output: output.text`A one-paragraph work summary.` })`
   */
  text<const Values extends readonly PromptInterpolation[]>(
    strings: TemplateStringsArray,
    ...values: Values
  ): InstructionOutputHandle<string>;
  /**
   * Builds a schema-typed instruction-output: like `output.text`, but the
   * auto-declared slot's schema is `schemaRef` and the rendered
   * return-format instruction names it.
   * @example `sequence.prompt("review", { ..., output: output.of(ref.schema("code-review-scaffold"))`Your verdict, citing every finding.` })`
   */
  of<Value = unknown>(
    schemaRef: SchemaValue<Value>,
  ): <const Values extends readonly PromptInterpolation[]>(
    strings: TemplateStringsArray,
    ...values: Values
  ) => InstructionOutputHandle<Value>;
  /**
   * Builds an output template: the prose is the attaching step's
   * instruction (appended to its compiled prompt), and each interpolated
   * slot IS the step's output contract — instructions and contract cannot
   * drift apart, the output-side mirror of `input.prompt`. Write
   * `${slot.optional()}` for an interpolation that lowers to an optional
   * output sink (P105). At least one interpolation is required; a slot
   * interpolated by two steps' `output.prompt` for different `agent:`
   * values is a build error.
   * @example `output.prompt`State your verdict in ${verdict} and your reasoning in ${reasoning.optional()}.``
   */
  prompt<const Values extends readonly OutputPromptInterpolation[]>(
    strings: TemplateStringsArray,
    ...values: Values
  ): OutputTemplateHandle;
}

/** Values that can be interpolated into an `output.prompt` output template. */
export type OutputPromptInterpolation<Value = unknown> = SlotHandle<Value> | OptionalSlotRead<Value>;

function buildInstructionOutput(
  strings: TemplateStringsArray,
  values: readonly PromptInterpolation[],
  schema: { readonly ref: string; readonly value: SchemaValue; } | undefined,
): InstructionOutputHandle {
  const built = promptTemplate(strings, values);
  const builtMeta = metaOf(built);
  const text = typeof built.text === "string" ? built.text : "";
  const refs = builtMeta?.refs;
  const optionalRefs = builtMeta?.optionalRefs;
  const declarations = mergeDeclarationSets(
    builtMeta?.declarations ?? {},
    schema === undefined ? {} : collectMany([schema.value]),
  );
  return withMeta({}, {
    kind: "instruction-output",
    instructionOutput: {
      text,
      ...(schema === undefined ? {} : { schemaRef: schema.ref }),
      ...(refs === undefined || refs.length === 0 ? {} : { refs }),
      ...(optionalRefs === undefined || optionalRefs.length === 0 ? {} : { optionalRefs }),
    },
    declarations,
  }) as InstructionOutputHandle;
}

/**
 * Resolves one `output.prompt` interpolation site to its slot ref, whether
 * optionality was written, and the underlying resolved value (kept so the
 * attaching step can later claim authorship against the actual slot handle
 * rather than only its ref string).
 */
function resolveOutputPromptRef(
  value: OutputPromptInterpolation,
  fieldPath: string,
): { readonly ref: string; readonly optional: boolean; readonly slotValue: unknown; } {
  const wrapper = value as { readonly slot?: unknown; readonly optional?: unknown; };
  if (
    value !== null && typeof value === "object" && !Array.isArray(value)
    && "slot" in wrapper && wrapper.optional === true
  ) {
    const ref = refText(wrapper.slot, fieldPath);
    if (!ref.startsWith("slot:")) throw new Error(`${fieldPath}: optional() applies to slot refs only`);
    return { ref, optional: true, slotValue: wrapper.slot };
  }
  const ref = refText(value, fieldPath);
  if (!ref.startsWith("slot:")) throw new Error(`${fieldPath}: output.prompt interpolation must be a slot reference`);
  return { ref, optional: false, slotValue: value };
}

function buildOutputTemplate(
  strings: TemplateStringsArray,
  values: readonly OutputPromptInterpolation[],
): OutputTemplateHandle {
  let text = strings[0] ?? "";
  const refs: string[] = [];
  const optionalRefs: string[] = [];
  const slotByRef = new Map<string, unknown>();
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (value === undefined) throw new Error(`output.prompt interpolation ${index}: expected a slot reference`);
    const { ref, optional, slotValue } = resolveOutputPromptRef(value, `output.prompt interpolation ${index}`);
    if (!refs.includes(ref)) refs.push(ref);
    if (optional && !optionalRefs.includes(ref)) optionalRefs.push(ref);
    slotByRef.set(ref, slotValue);
    // Rendered as bare prose, never a `{ref}` interpolation span: the
    // canonical prompt contract treats `{ref}` as an unconditional READ
    // contract (prompt_contract.rs), but an output-template slot is a WRITE
    // contract carried by `output:`/`refs` — interpolating it here would make
    // the step read its own not-yet-produced output.
    text += `${ref}${strings[index + 1] ?? ""}`;
  }
  if (refs.length === 0) {
    throw new Error(
      "output.prompt: expected at least one interpolated slot — a template with zero interpolations has no output contract",
    );
  }
  return withMeta({}, {
    kind: "output-template",
    outputTemplate: {
      text,
      refs,
      ...(optionalRefs.length === 0 ? {} : { optionalRefs }),
      slots: refs.map((ref) => slotByRef.get(ref)),
    },
    declarations: collectMany(values),
  }) as OutputTemplateHandle;
}

/** Sequence-step output authoring markers. @see {@link OutputFunction} */
export const output: OutputFunction = {
  text: (strings: TemplateStringsArray, ...values: readonly PromptInterpolation[]) =>
    buildInstructionOutput(strings, values, undefined) as InstructionOutputHandle<string>,
  of: (schemaRef: SchemaValue) => {
    const resolvedSchemaRef = refText(schemaRef, "output.of.schema");
    return (strings: TemplateStringsArray, ...values: readonly PromptInterpolation[]) =>
      buildInstructionOutput(strings, values, { ref: resolvedSchemaRef, value: schemaRef });
  },
  prompt: (strings: TemplateStringsArray, ...values: readonly OutputPromptInterpolation[]) =>
    buildOutputTemplate(strings, values),
} as OutputFunction;

export { attachInstructionOutput, instructionOutputContent, isInstructionOutputHandle };
