import type {
  InstructionOutputHandle,
  OptionalSlotRead,
  OutputPromptInterpolation,
  OutputTemplateHandle,
  SlotHandle,
} from "./handles.js";
import {
  attachInstructionOutput,
  instructionOutputContent,
  isInstructionOutputHandle,
  metaOf,
  withHiddenField,
  withMeta,
} from "./meta.js";
import { collectMany, dedentPromptText, mergeDeclarationSets, uniqueInOrder } from "./normalize.js";
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
export type { OutputPromptInterpolation } from "./handles.js";

function buildInstructionOutput(
  strings: TemplateStringsArray,
  values: readonly PromptInterpolation[],
  schema: { readonly ref: string; readonly value: SchemaValue } | undefined,
): InstructionOutputHandle {
  const built = promptTemplate(strings, values);
  const builtMeta = metaOf(built);
  // `promptTemplate` already dedented; this reads the dedented value back.
  const text = typeof built.text === "string" ? built.text : "";
  const refs = builtMeta?.refs;
  const optionalRefs = builtMeta?.optionalRefs;
  const declarations = mergeDeclarationSets(
    builtMeta?.declarations ?? {},
    schema === undefined ? {} : collectMany([schema.value]),
  );
  return withMeta(
    {},
    {
      kind: "instruction-output",
      instructionOutput: {
        text,
        ...(schema === undefined ? {} : { schemaRef: schema.ref }),
        ...(refs === undefined || refs.length === 0 ? {} : { refs }),
        ...(optionalRefs === undefined || optionalRefs.length === 0 ? {} : { optionalRefs }),
      },
      declarations,
    },
  ) as InstructionOutputHandle;
}

/**
 * Resolves one `output.prompt` interpolation site: a slot ref (whether
 * optionality was written, and the underlying resolved value, kept so the
 * attaching step can later claim authorship against the actual slot handle
 * rather than only its ref string), or a signal ref — interpolating a
 * signal itself IS that step's may-emit declaration (0253.2), not a WRITE
 * contract, so it carries no optionality/authorship concept of its own.
 */
function resolveOutputPromptRef(
  value: OutputPromptInterpolation,
  fieldPath: string,
):
  | { readonly kind: "slot"; readonly ref: string; readonly optional: boolean; readonly slotValue: SlotHandle }
  | { readonly kind: "signal"; readonly ref: string } {
  const wrapper = value as OptionalSlotRead;
  if (
    value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    "slot" in wrapper &&
    wrapper.optional === true
  ) {
    const ref = refText(wrapper.slot, fieldPath);
    if (!ref.startsWith("slot:")) throw new Error(`${fieldPath}: optional() applies to slot refs only`);
    return { kind: "slot", ref, optional: true, slotValue: wrapper.slot };
  }
  const ref = refText(value, fieldPath);
  if (ref.startsWith("signal:")) return { kind: "signal", ref };
  if (!ref.startsWith("slot:")) {
    throw new Error(`${fieldPath}: output.prompt interpolation must be a slot or signal reference`);
  }
  return { kind: "slot", ref, optional: false, slotValue: value as SlotHandle };
}

interface OutputTemplateParts {
  readonly text: string;
  readonly refs: readonly string[];
  readonly optionalRefs: readonly string[];
  readonly slotByRef: ReadonlyMap<string, SlotHandle>;
  /** Signals interpolated in this template, in interpolation order — a SEPARATE list from `refs`: every existing `refs`/`slots` consumer treats `refs` as the step's slot output contract, and a signal is not one (P565/0253.2). */
  readonly signals: readonly string[];
  readonly declarations: ReturnType<typeof collectMany>;
}

function outputTemplateParts(
  strings: TemplateStringsArray,
  values: readonly OutputPromptInterpolation[],
): OutputTemplateParts {
  let text = strings[0] ?? "";
  const refs: string[] = [];
  const optionalRefs: string[] = [];
  const slotByRef = new Map<string, SlotHandle>();
  const signals: string[] = [];
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (value === undefined) {
      throw new Error(`output.prompt interpolation ${index}: expected a slot or signal reference`);
    }
    const resolved = resolveOutputPromptRef(value, `output.prompt interpolation ${index}`);
    if (resolved.kind === "signal") {
      if (!signals.includes(resolved.ref)) signals.push(resolved.ref);
      // Signals render BRACED (`{signal:...}`), unlike a slot ref (see
      // below): `frame_prompt.rs`'s `authored_signal_refs` scans the
      // compiled prompt for exactly this token to render the emission
      // guidance into the step's `<spec>` block, and a signal interpolation
      // is exempt from the slot read-contract rule — it's the may-emit
      // declaration itself, not a step reading its own output.
      text += `{${resolved.ref}}${strings[index + 1] ?? ""}`;
      continue;
    }
    const { ref, optional, slotValue } = resolved;
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
  return { text, refs, optionalRefs, slotByRef, signals, declarations: collectMany(values) };
}

/** Merges a base output-template's parts with an extension's, per the same required-beats-optional rule `prompt.ts`'s `composeTemplateMeta` applies. */
function composeOutputTemplateParts(base: OutputTemplateParts, extension: OutputTemplateParts): OutputTemplateParts {
  const refs = uniqueInOrder([...base.refs, ...extension.refs]);
  const requiredRefs = new Set([
    ...base.refs.filter((ref) => !base.optionalRefs.includes(ref)),
    ...extension.refs.filter((ref) => !extension.optionalRefs.includes(ref)),
  ]);
  const optionalRefs = uniqueInOrder([...base.optionalRefs, ...extension.optionalRefs]).filter(
    (ref) => !requiredRefs.has(ref),
  );
  const slotByRef = new Map([...base.slotByRef, ...extension.slotByRef]);
  const signals = uniqueInOrder([...base.signals, ...extension.signals]);
  return {
    text: `${base.text}\n${extension.text}`,
    refs,
    optionalRefs,
    slotByRef,
    signals,
    declarations: mergeDeclarationSets(base.declarations, extension.declarations),
  };
}

function finishOutputTemplate(parts: OutputTemplateParts): OutputTemplateHandle {
  if (parts.refs.length === 0 && parts.signals.length === 0) {
    throw new Error(
      "output.prompt: expected at least one interpolated slot or signal — a template with zero interpolations has no output contract",
    );
  }
  const template = withMeta(
    {},
    {
      kind: "output-template",
      outputTemplate: {
        text: dedentPromptText(parts.text),
        refs: parts.refs,
        ...(parts.optionalRefs.length === 0 ? {} : { optionalRefs: parts.optionalRefs }),
        slots: parts.refs.map((ref) => parts.slotByRef.get(ref)),
        ...(parts.signals.length === 0 ? {} : { signals: parts.signals }),
      },
      declarations: parts.declarations,
    },
  ) as OutputTemplateHandle;
  const extend = (
    strings: TemplateStringsArray,
    ...values: readonly OutputPromptInterpolation[]
  ): OutputTemplateHandle =>
    // The zero-interpolation throw applies to the FULL composed contract,
    // not the extension part in isolation — a prose-only doctrine
    // extension is legal because the base already carries the contract.
    finishOutputTemplate(composeOutputTemplateParts(parts, outputTemplateParts(strings, values)));
  return withHiddenField(template, "extend", extend);
}

function buildOutputTemplate(
  strings: TemplateStringsArray,
  values: readonly OutputPromptInterpolation[],
): OutputTemplateHandle {
  return finishOutputTemplate(outputTemplateParts(strings, values));
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
