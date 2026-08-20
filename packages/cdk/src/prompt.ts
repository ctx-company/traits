import type { JsonObject } from "./generated.js";
import type {
  OptionalSlotRead,
  PortHandle,
  PromptHandle,
  PromptInterpolation,
  PromptTemplate,
  RefHandle,
  ResourceHandle,
  SettingHandle,
  SlotHandle,
} from "./handles.js";
import type { DeclKind } from "./meta.js";
import { metaOf, withDeclaration, withHiddenField, withMeta } from "./meta.js";
import {
  collectMany,
  compact,
  dedentPromptText,
  mergeDeclarationSets,
  normalizeRefList,
  slugFromName,
  unique,
  uniqueInOrder,
  validateSlug,
} from "./normalize.js";

export interface PromptFields {
  readonly id: string;
  readonly description?: string;
  readonly text?: string | PromptTemplate;
  readonly input?:
    | PortHandle
    | SlotHandle
    | SettingHandle
    | ResourceHandle
    | readonly (PortHandle | SlotHandle | SettingHandle | ResourceHandle)[];
  readonly output?: SlotHandle | readonly SlotHandle[];
  readonly source?: string | ResourceHandle | RefHandle | PromptTemplate;
}

export type { PromptInterpolation } from "./handles.js";

/** `prompt.resource(...)`'s accepted shape: a bare resource ref, or a resource plus its own extra declared inputs. */
export type PromptResourceValue =
  | ResourceHandle
  | RefHandle
  | {
      readonly resource: ResourceHandle | RefHandle | string;
      readonly input?: PromptFields["input"];
    };

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
  resource<Input = unknown>(value: PromptResourceValue): PromptTemplate<Input>;
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
export const prompt = Object.assign(promptFn, {
  resource: (value: PromptResourceValue): PromptTemplate => promptResource(value),
}) as PromptFunction;

function promptOf(fields: PromptFields): PromptHandle {
  const id = slugFromName(fields.id, "prompt.id");
  const text = promptTextValue(fields.text);
  const source =
    fields.source === undefined
      ? promptSourceFromText(fields.text, `prompt.${id}.text`)
      : promptSourceValue(fields.source, `prompt.${id}.source`);
  if (text !== undefined && source !== undefined) {
    throw new Error(`prompt.${id}: expected either text prompt body or source, not both`);
  }
  const refs = promptRefsForFields(fields);
  const declarations = collectMany([fields.input, fields.output, fields.source, fields.text]);
  const declaration = compact({
    id,
    input: normalizeRefList(fields.input ?? refs),
    output: normalizeRefList(fields.output),
    description: fields.description,
    text,
    source,
  });
  return withDeclaration("prompt", `prompt:${id}`, declaration, {}, { declarations, refs });
}

/** One template's mergeable facets — shared shape between a literal/interpolated span and a whole template. */
interface TemplateFacets {
  readonly text: string;
  readonly refs: readonly string[];
  readonly optionalRefs: readonly string[];
  readonly declarations: Partial<Record<DeclKind, readonly JsonObject[]>>;
}

/**
 * Merges ordered template facets: `refs` union in first-appearance order,
 * `optionalRefs` union with required-beats-optional on collision (a ref
 * required in ANY part is never optional in the result), `declarations`
 * concatenation via `mergeDeclarationSets`. Shared by `.extend`, nested
 * template interpolation, and `output.prompt` composition — the substance
 * of 0166.
 */
function composeTemplateMeta(parts: readonly TemplateFacets[], joiner: string): TemplateFacets {
  const refs = uniqueInOrder(parts.flatMap((part) => part.refs));
  const requiredRefs = new Set(parts.flatMap((part) => part.refs.filter((ref) => !part.optionalRefs.includes(ref))));
  const optionalRefs = uniqueInOrder(parts.flatMap((part) => part.optionalRefs)).filter(
    (ref) => !requiredRefs.has(ref),
  );
  return {
    text: parts.map((part) => part.text).join(joiner),
    refs,
    optionalRefs,
    declarations: mergeDeclarationSets(...parts.map((part) => part.declarations)),
  };
}

/** A template's facets read off its meta — text-less (`prompt.resource`) templates throw at the call site named by `fieldPath`. */
function templateFacets(value: PromptTemplate, fieldPath: string): TemplateFacets {
  const meta = metaOf(value);
  const text = (value as unknown as { readonly text?: unknown }).text;
  if (typeof text !== "string") {
    throw new Error(`${fieldPath}: expected a prompt template with rendered text, got a resource-backed template`);
  }
  return {
    text,
    refs: meta?.refs ?? [],
    optionalRefs: meta?.optionalRefs ?? [],
    declarations: meta?.declarations ?? {},
  };
}

/**
 * Builds a `PromptTemplate` from a tagged template literal — the shared
 * builder behind `input.prompt` (input.ts re-exports this under that name
 * rather than duplicating it, to avoid an import cycle: `input.ts`
 * otherwise imports only `handles.ts`/`normalize.ts`/`ref.ts`).
 */
export function promptTemplate(strings: TemplateStringsArray, values: readonly PromptInterpolation[]): PromptTemplate {
  const parts: TemplateFacets[] = [{ text: strings[0] ?? "", refs: [], optionalRefs: [], declarations: {} }];
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (value === undefined) throw new Error(`input.prompt interpolation ${index}: expected a reference`);
    const literal = strings[index + 1] ?? "";
    if (metaOf(value)?.kind === "template") {
      const inner = templateFacets(value as PromptTemplate, `input.prompt interpolation ${index}`);
      parts.push({ ...inner, text: inner.text + literal });
      continue;
    }
    const { ref, optional } = resolvePromptRef(value, `input.prompt interpolation ${index}`);
    parts.push({
      text: `{${ref}}${literal}`,
      refs: [ref],
      // `${slot.optional()}` lowers to an optional step input (P447), same
      // as an optional include: the runtime renders `[ref]` against a frame
      // with no element for it — a reference to something not present,
      // never an unresolved token — so round-1 loop steps can reference
      // later rounds' slots in place.
      optionalRefs: optional ? [ref] : [],
      // Unwrap `${slot.optional()}` markers so a slot whose only appearance
      // is an optional interpolation still carries its declaration.
      declarations: collectMany([isOptionalSlotRead(value) ? value.slot : value]),
    });
  }
  const composed = composeTemplateMeta(parts, "");
  return buildPromptTemplate(composed);
}

/**
 * Binds named `{placeholder}` references in existing prompt text — the
 * non-tagged counterpart for prompts assembled from static doctrines. The
 * shared builder behind `input.prompt`'s non-tagged overload.
 */
export function promptBoundText(text: string, bindings: Readonly<Record<string, PromptInterpolation>>): PromptTemplate {
  const values = Object.values(bindings);
  const optionalRefs: string[] = [];
  const refs = Object.fromEntries(
    Object.entries(bindings).map(([name, value]) => {
      const { ref, optional } = resolvePromptRef(value, `input.prompt ${name}`);
      if (optional && !optionalRefs.includes(ref)) optionalRefs.push(ref);
      return [name, ref];
    }),
  );
  const rendered = text.replace(/\{([A-Za-z][A-Za-z0-9-]*)\}/g, (placeholder, name: string) =>
    refs[name] === undefined ? placeholder : `{${refs[name]}}`,
  );
  return buildPromptTemplate({
    text: rendered,
    refs: [...new Set(Object.values(refs))],
    optionalRefs,
    declarations: collectMany(values.map((value) => (isOptionalSlotRead(value) ? value.slot : value))),
  });
}

/** Attaches the hidden, non-enumerable `.extend` member every composable `PromptTemplate` carries. */
function buildPromptTemplate(facets: TemplateFacets): PromptTemplate {
  // Dedented here, at the one point composed text becomes a canonical value,
  // and NOT in `facets` — `extend` below composes from the raw facets, so a
  // chained template dedents once over the whole composition rather than
  // per-part, where each part's own indent would be measured separately.
  const text = dedentPromptText(facets.text);
  const template = withMeta<{ text: string }, "template", unknown, "prompt-template">(
    { text },
    {
      kind: "template",
      refs: [...new Set(facets.refs)],
      ...(facets.optionalRefs.length === 0 ? {} : { optionalRefs: [...new Set(facets.optionalRefs)] }),
      declaration: { text },
      declarations: facets.declarations,
    },
  );
  const extend = (strings: TemplateStringsArray, ...values: readonly PromptInterpolation[]): PromptTemplate => {
    const extension = promptTemplate(strings, values);
    const composed = composeTemplateMeta([facets, templateFacets(extension, "prompt.extend")], "\n");
    return buildPromptTemplate(composed);
  };
  return withHiddenField(template, "extend", extend);
}

/**
 * One interpolation site's ref plus whether it was written optionally
 * (`slot.optional()` / `input.optional(slot)`). Optionality is per-site, so
 * it is resolved here and carried to the step's `input` array — never onto
 * the slot declaration itself.
 */
function isOptionalSlotRead(value: unknown): value is OptionalSlotRead {
  return (
    value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    "slot" in value &&
    (value as { readonly optional?: unknown }).optional === true
  );
}

function resolvePromptRef(value: PromptInterpolation, fieldPath: string): { ref: string; optional: boolean } {
  if (isOptionalSlotRead(value)) {
    const inner = normalizeRefList(value.slot);
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

function promptResource(value: PromptResourceValue): PromptTemplate {
  const resource =
    "resource" in (value as object)
      ? (value as { readonly resource: ResourceHandle | RefHandle | string }).resource
      : (value as ResourceHandle | RefHandle);
  const input = "input" in (value as object) ? (value as { readonly input?: PromptFields["input"] }).input : undefined;
  const resourceRef = resourceSourceRef(resource, "prompt.resource.resource");
  const refs = input === undefined ? [resourceRef] : [resourceRef, ...(normalizeRefList(input) ?? [])];
  const template = withMeta<{ source: string; input: string[] | undefined }, "template", unknown, "prompt-template">(
    { source: resourceRef, input: normalizeRefList(input) },
    {
      kind: "template",
      refs: unique(refs),
      declaration: { source: resourceRef },
      declarations: collectMany([resource, input]),
    },
  );
  const throwUnextendable = (): never => {
    throw new Error("prompt.resource templates cannot be extended — the body is resource content");
  };
  return withHiddenField(template, "extend", throwUnextendable);
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
  const textRefs =
    fields.text === undefined || typeof fields.text === "string" ? [] : (metaOf(fields.text)?.refs ?? []);
  const sourceRefs =
    fields.source === undefined
      ? []
      : typeof fields.source === "string"
        ? [normalizeResourceSourceString(fields.source, "prompt.source")]
        : (metaOf(fields.source)?.refs ??
          (metaOf(fields.source)?.ref === undefined ? [] : [metaOf(fields.source)?.ref as string]));
  return unique([...textRefs, ...sourceRefs]);
}
function resourceSourceRef(value: ResourceHandle | RefHandle | string, fieldPath: string): string {
  return normalizeResourceSourceString(typeof value === "string" ? value : (metaOf(value)?.ref ?? ""), fieldPath);
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
