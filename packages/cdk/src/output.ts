import type { InstructionOutputHandle } from "./handles.js";
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
}

function buildInstructionOutput(
  strings: TemplateStringsArray,
  values: readonly PromptInterpolation[],
  schema: { readonly ref: string; readonly value: SchemaValue; } | undefined,
): InstructionOutputHandle {
  const built = promptTemplate(strings, values) as unknown as { readonly text?: unknown; };
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

/** Sequence-step output authoring markers. @see {@link OutputFunction} */
export const output: OutputFunction = {
  text: (strings: TemplateStringsArray, ...values: readonly PromptInterpolation[]) =>
    buildInstructionOutput(strings, values, undefined) as InstructionOutputHandle<string>,
  of: (schemaRef: SchemaValue) => {
    const resolvedSchemaRef = refText(schemaRef, "output.of.schema");
    return (strings: TemplateStringsArray, ...values: readonly PromptInterpolation[]) =>
      buildInstructionOutput(strings, values, { ref: resolvedSchemaRef, value: schemaRef });
  },
} as OutputFunction;

export { attachInstructionOutput, instructionOutputContent, isInstructionOutputHandle };
