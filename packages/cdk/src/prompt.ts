import type { JsonObject } from "./generated.js";
import type {
  InstructionOutputHandle,
  PortHandle,
  PromptHandle,
  PromptTemplate,
  RefHandle,
  ResourceHandle,
  SlotHandle,
} from "./handles.js";
import { metaOf, withDeclaration, withMeta } from "./meta.js";
import { collectMany, compact, normalizeRefList, unique, validateSlug } from "./normalize.js";

export interface PromptFields {
  readonly id: string;
  readonly description?: string;
  readonly text?: string | PromptTemplate;
  readonly input?: PortHandle | SlotHandle | ResourceHandle | readonly (PortHandle | SlotHandle | ResourceHandle)[];
  readonly output?: SlotHandle | readonly SlotHandle[];
  readonly source?: string | ResourceHandle | RefHandle | PromptTemplate;
}

/** Values that can be interpolated into a prompt and therefore become prompt inputs. */
export type PromptInterpolation<Value = unknown> =
  | PortHandle<Value>
  | SlotHandle<Value>
  | ResourceHandle<Value>
  | InstructionOutputHandle<Value>;

export interface PromptFunction {
  <Input, Output = unknown>(
    fields: PromptFields & {
      readonly text: PromptTemplate<Input>;
      readonly input?: PromptFields["input"];
      readonly output?: SlotHandle<Output> | readonly SlotHandle<Output>[];
    },
  ): PromptHandle<Input, Output>;
  <Input = unknown, Output = unknown>(
    fields: PromptFields & {
      readonly input?: PromptFields["input"];
      readonly output?: SlotHandle<Output> | readonly SlotHandle<Output>[];
    },
  ): PromptHandle<Input, Output>;
  (id: string, text: string, fields?: JsonObject): PromptHandle;
  /**
   * Builds a `PromptTemplate` whose body is a resource's content, with the
   * resource itself as the prompt's declared input — the equivalent of
   * `input.prompt` for prompt bodies that live in their own file/inline
   * resource instead of a literal in the trait source.
   * @example
   * ```ts
   * const styleGuideResource = resource.inline("style-guide", "Prefer minimal diffs.");
   * prompt.resource(styleGuideResource);
   * ```
   */
  resource<Input = unknown>(
    value: ResourceHandle | RefHandle | {
      readonly resource: ResourceHandle | RefHandle | string;
      readonly input?: readonly unknown[];
    },
  ): PromptTemplate<Input>;
}

/**
 * Declares prompts and typed prompt templates: the text sent to an agent,
 * with interpolated ports/slots/resources tracked as typed inputs instead
 * of raw string concatenation.
 *
 * A hand-written prompt body is an opaque string — the runtime cannot tell
 * which slots or ports it actually reads, so it can neither validate nor
 * inject them correctly. Building the text with `input.prompt`
 * (or passing one to `sequence.prompt`'s `input:` field directly, which is
 * the common path — declaring a standalone named `prompt(...)` is only
 * needed when the same prompt is reused across steps) means every
 * interpolation is a typed reference, and a stale reference to a renamed
 * slot fails at `tsc`.
 *
 * @param fields Prompt declaration fields.
 * @example
 * ```ts
 * const request = port.input.text({ id: "request" });
 * input.prompt`Summarize ${request}`;
 * ```
 * @see {@link sequence}
 */
function promptFn<Input, Output = unknown>(
  fields: PromptFields & {
    readonly text: PromptTemplate<Input>;
    readonly input?: PromptFields["input"];
    readonly output?: SlotHandle<Output> | readonly SlotHandle<Output>[];
  },
): PromptHandle<Input, Output>;
function promptFn<Input = unknown, Output = unknown>(
  fields: PromptFields & {
    readonly input?: PromptFields["input"];
    readonly output?: SlotHandle<Output> | readonly SlotHandle<Output>[];
  },
): PromptHandle<Input, Output>;
function promptFn(id: string, text: string, fields?: JsonObject): PromptHandle;
function promptFn(idOrFields: string | PromptFields, text?: string, fields?: JsonObject): PromptHandle {
  return typeof idOrFields === "string"
    ? promptOf({ ...fields, id: idOrFields, text: text as string })
    : promptOf(idOrFields);
}

// `text` stays a generic interface member compared against a plain
// (non-generic) implementation here — the same `Object.assign`
// generic-merge limitation `port.ts`/`slot.ts` document — so the namespace
// as a whole still binds through one cast; `promptFn` itself (the part that
// used to read `args[n]`) is now a real checked overloaded function.
export const prompt = Object.assign(
  promptFn,
  {
    resource: (
      value: ResourceHandle | RefHandle | {
        readonly resource: ResourceHandle | RefHandle | string;
        readonly input?: readonly unknown[];
      },
    ): PromptTemplate => promptResource(value),
  },
) as PromptFunction;

function promptOf(fields: PromptFields): PromptHandle {
  validateSlug(fields.id, "prompt.id");
  const text = promptTextValue(fields.text);
  const source = fields.source === undefined
    ? promptSourceFromText(fields.text, `prompt.${fields.id}.text`)
    : promptSourceValue(fields.source, `prompt.${fields.id}.source`);
  if (text !== undefined && source !== undefined) {
    throw new Error(`prompt.${fields.id}: expected either text prompt body or source, not both`);
  }
  const refs = promptRefsForFields(fields);
  const declarations = collectMany([fields.input, fields.output, fields.source, fields.text]);
  const declaration = compact({
    id: fields.id,
    input: normalizeRefList(fields.input ?? refs),
    output: normalizeRefList(fields.output),
    description: fields.description,
    text,
    source,
  });
  return withDeclaration("prompt", `prompt:${fields.id}`, declaration, {}, { declarations, refs });
}

/**
 * Builds a `PromptTemplate` from a tagged template literal — the shared
 * builder behind `input.prompt` (input.ts re-exports this under that name
 * rather than duplicating it, to avoid an import cycle: `input.ts`
 * otherwise imports only `handles.ts`/`normalize.ts`/`ref.ts`).
 */
export function promptTemplate(
  strings: TemplateStringsArray,
  values: readonly PromptInterpolation[],
): PromptTemplate {
  let text = strings[0] ?? "";
  const refs: string[] = [];
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (value === undefined) throw new Error(`input.prompt interpolation ${index}: expected a reference`);
    const ref = requirePromptRef(value, `input.prompt interpolation ${index}`);
    refs.push(ref);
    text += `{${ref}}${strings[index + 1] ?? ""}`;
  }
  return withMeta({ text }, {
    kind: "template",
    refs: [...new Set(refs)],
    declaration: { text },
    declarations: collectMany(values),
  });
}

/**
 * Binds named `{placeholder}` references in existing prompt text — the
 * non-tagged counterpart for prompts assembled from static doctrines. The
 * shared builder behind `input.prompt`'s non-tagged overload.
 */
export function promptBoundText(text: string, bindings: Readonly<Record<string, PromptInterpolation>>): PromptTemplate {
  const values = Object.values(bindings);
  const refs = Object.fromEntries(
    Object.entries(bindings).map(([name, value]) => [name, requirePromptRef(value, `input.prompt ${name}`)]),
  );
  const rendered = text.replace(
    /\{([A-Za-z][A-Za-z0-9-]*)\}/g,
    (placeholder, name: string) => refs[name] === undefined ? placeholder : `{${refs[name]}}`,
  );
  return withMeta({ text: rendered }, {
    kind: "template",
    refs: [...new Set(Object.values(refs))],
    declaration: { text: rendered },
    declarations: collectMany(values),
  });
}

function requirePromptRef(value: PromptInterpolation, fieldPath: string): string {
  return resolvePromptRef(value, fieldPath).ref;
}

/**
 * One interpolation site's ref plus whether it was written optionally
 * (`slot.optional()` / `input.optional(slot)`). Optionality is per-site, so
 * it is resolved here and carried to the step's `input` array — never onto
 * the slot declaration itself.
 */
function resolvePromptRef(
  value: PromptInterpolation,
  fieldPath: string,
): { ref: string; optional: boolean; } {
  const wrapper = value as { readonly slot?: unknown; readonly optional?: unknown; };
  if (
    value !== null && typeof value === "object" && !Array.isArray(value)
    && "slot" in wrapper && wrapper.optional === true
  ) {
    const inner = normalizeRefList(wrapper.slot as PromptInterpolation);
    if (inner?.[0] === undefined) throw new Error(`${fieldPath}: expected a slot reference`);
    if (!inner[0].startsWith("slot:")) throw new Error(`${fieldPath}: optional() applies to slot refs only`);
    return { ref: inner[0], optional: true };
  }
  // An `output.text`/`output.of(...)` instruction-output resolves eagerly
  // too, but its ref isn't known until the step that lists it in `output:`
  // attaches it (`meta.ts`'s `attachInstructionOutput` mutates the SAME meta
  // object in place) — so interpolating one here only works once its
  // producing step has already been authored (the natural order: produce,
  // then consume). Interpolating an instruction-output before it's ever
  // attached is exactly the "never attached" authoring bug the task calls
  // out — surfaced here, at the point of interpolation, rather than
  // deferred to a separate collection pass.
  if (metaOf(value)?.kind === "instruction-output") {
    const ref = metaOf(value)?.ref;
    if (typeof ref !== "string") {
      throw new Error(
        `${fieldPath}: output.text/output.of value interpolated before the step that lists it in output: was authored`,
      );
    }
    return { ref, optional: false };
  }
  const normalized = normalizeRefList(value);
  if (normalized?.[0] === undefined) throw new Error(`${fieldPath}: expected a reference`);
  return { ref: normalized[0], optional: false };
}

function promptResource(
  value: ResourceHandle | RefHandle | {
    readonly resource: ResourceHandle | RefHandle | string;
    readonly input?: readonly unknown[];
  },
): PromptTemplate {
  const resource = "resource" in (value as object)
    ? (value as { readonly resource: ResourceHandle | RefHandle | string; }).resource
    : value as ResourceHandle | RefHandle;
  const input = "input" in (value as object) ? (value as { readonly input?: readonly unknown[]; }).input : undefined;
  const resourceRef = resourceSourceRef(resource, "prompt.resource.resource");
  const refs = input === undefined ? [resourceRef] : [resourceRef, ...(normalizeRefList(input) ?? [])];
  return withMeta({ source: resourceRef, input: normalizeRefList(input) }, {
    kind: "template",
    refs: unique(refs),
    declaration: { source: resourceRef },
    declarations: collectMany([resource, input]),
  });
}

function promptTextValue(value: PromptFields["text"]): string | undefined {
  if (value === undefined || typeof value === "string") return value;
  return typeof value.text === "string" ? value.text : undefined;
}
function promptSourceFromText(value: PromptFields["text"], fieldPath: string): string | undefined {
  return value === undefined || typeof value === "string" ? undefined : promptSourceValue(value, fieldPath);
}
function promptSourceValue(
  value: PromptFields["source"] | PromptFields["text"],
  fieldPath: string,
): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value === "string") return normalizeResourceSourceString(value, fieldPath);
  if (typeof value.source === "string") return value.source;
  const meta = metaOf(value);
  return meta?.kind === "resource" || meta?.ref?.startsWith("resource:") ? meta.ref : undefined;
}
function promptRefsForFields(fields: PromptFields): readonly string[] {
  const textRefs = fields.text === undefined || typeof fields.text === "string" ? [] : metaOf(fields.text)?.refs ?? [];
  const sourceRefs = fields.source === undefined
    ? []
    : typeof fields.source === "string"
    ? [normalizeResourceSourceString(fields.source, "prompt.source")]
    : metaOf(fields.source)?.refs
      ?? (metaOf(fields.source)?.ref === undefined ? [] : [metaOf(fields.source)?.ref as string]);
  return unique([...textRefs, ...sourceRefs]);
}
function resourceSourceRef(value: ResourceHandle | RefHandle | string, fieldPath: string): string {
  return normalizeResourceSourceString(typeof value === "string" ? value : metaOf(value)?.ref ?? "", fieldPath);
}
function normalizeResourceSourceString(value: string, fieldPath: string): string {
  const trimmed = value.trim();
  if (trimmed !== value || value.length === 0 || /\s|[{}]/.test(value)) {
    throw new Error(`${fieldPath}: expected resource id or resource ref, got ${JSON.stringify(value)}`);
  }
  const match = /^([a-z]+):(.*)$/.exec(value);
  if (match !== null) {
    if (match[1] !== "resource") throw new Error(`${fieldPath}: expected resource ref, got ${JSON.stringify(value)}`);
    return value;
  }
  validateSlug(value, fieldPath);
  return `resource:${value}`;
}
