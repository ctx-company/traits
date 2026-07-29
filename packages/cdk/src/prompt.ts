import type { JsonObject, RefKind } from "./generated.js";
import type {
  Handle,
  PortHandle,
  PromptHandle,
  PromptTemplate,
  RefHandle,
  ResourceHandle,
  SlotHandle,
} from "./handles.js";
import { metaOf, recordDiagnostic, withDeclaration, withMeta } from "./meta.js";
import {
  collectMany,
  compact,
  normalizeRefList,
  REF_TOKEN_PATTERN,
  unique,
  uniqueInOrder,
  validateSlug,
} from "./normalize.js";

export interface PromptFields {
  readonly id: string;
  readonly description?: string;
  readonly text?: string | PromptTemplate;
  readonly input?: PortHandle | SlotHandle | ResourceHandle | readonly (PortHandle | SlotHandle | ResourceHandle)[];
  readonly output?: SlotHandle | readonly SlotHandle[];
  readonly source?: string | ResourceHandle | RefHandle | PromptTemplate;
}

/** Values that can be interpolated into a prompt and therefore become prompt inputs. */
export type PromptInterpolation<Value = unknown> = PortHandle<Value> | SlotHandle<Value> | ResourceHandle<Value>;
type HandleValue<Value> = Value extends Handle<RefKind, infer Result> ? Result : never;
type PromptInputs<Values extends readonly PromptInterpolation[]> = HandleValue<Values[number]>;
type TemplateName<Name extends string> = Name extends `${infer First}${infer Rest}` ? First extends
    | "A"
    | "B"
    | "C"
    | "D"
    | "E"
    | "F"
    | "G"
    | "H"
    | "I"
    | "J"
    | "K"
    | "L"
    | "M"
    | "N"
    | "O"
    | "P"
    | "Q"
    | "R"
    | "S"
    | "T"
    | "U"
    | "V"
    | "W"
    | "X"
    | "Y"
    | "Z"
    | "a"
    | "b"
    | "c"
    | "d"
    | "e"
    | "f"
    | "g"
    | "h"
    | "i"
    | "j"
    | "k"
    | "l"
    | "m"
    | "n"
    | "o"
    | "p"
    | "q"
    | "r"
    | "s"
    | "t"
    | "u"
    | "v"
    | "w"
    | "x"
    | "y"
    | "z"
    | "_"
    | "$" ? Rest extends "" ? Name
    : Rest extends `${infer Character}${infer Tail}` ? Character extends
        | "A"
        | "B"
        | "C"
        | "D"
        | "E"
        | "F"
        | "G"
        | "H"
        | "I"
        | "J"
        | "K"
        | "L"
        | "M"
        | "N"
        | "O"
        | "P"
        | "Q"
        | "R"
        | "S"
        | "T"
        | "U"
        | "V"
        | "W"
        | "X"
        | "Y"
        | "Z"
        | "a"
        | "b"
        | "c"
        | "d"
        | "e"
        | "f"
        | "g"
        | "h"
        | "i"
        | "j"
        | "k"
        | "l"
        | "m"
        | "n"
        | "o"
        | "p"
        | "q"
        | "r"
        | "s"
        | "t"
        | "u"
        | "v"
        | "w"
        | "x"
        | "y"
        | "z"
        | "_"
        | "$"
        | "0"
        | "1"
        | "2"
        | "3"
        | "4"
        | "5"
        | "6"
        | "7"
        | "8"
        | "9" ? TemplateName<`${First}${Tail}`> extends never ? never : Name
      : never
    : never
  : never
  : never;
type TemplateNames<Source extends string> = Source extends `${string}{${infer Name}}${infer Rest}`
  ? TemplateName<Name> | TemplateNames<Rest>
  : never;
type TemplateParams<Source extends string> = { readonly [Name in TemplateNames<Source>]: PromptInterpolation; };
type ExactTemplateParams<Source extends string, Params extends TemplateParams<Source>> =
  Exclude<keyof Params, TemplateNames<Source>> extends never ? Params : never;

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
   * Builds a `PromptTemplate` from a tagged template literal: each
   * interpolated `${handle}` becomes a typed `{port:*}`/`{slot:*}`/
   * `{resource:*}` reference token, and the referenced handle is
   * auto-declared as the prompt's input — no separate `input: [...]` list to
   * keep in sync by hand.
   * @example
   * ```ts
   * const diff = port.input.text({ id: "diff" });
   * const focus = port.input.text({ id: "focus" });
   * prompt.text`Review ${diff} with a focus on ${focus}.`;
   * ```
   */
  text<const Values extends readonly PromptInterpolation[]>(
    strings: TemplateStringsArray,
    ...values: Values
  ): PromptTemplate<PromptInputs<Values>>;
  /**
   * Builds a `PromptTemplate` whose body is a resource's content, with the
   * resource itself as the prompt's declared input — the equivalent of
   * `prompt.text` for prompt bodies that live in their own file/inline
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
  /**
   * Builds a `PromptTemplate` from a `{name}`-placeholder source string (or
   * resource) plus a params map binding each placeholder to a typed handle.
   *
   * A tagged-template `prompt.text` interpolation site is positional and
   * anonymous; `prompt.template` names each placeholder instead, which is
   * the shape long, multi-paragraph prompt bodies need — the same handle can
   * appear in the text multiple times under one name, and every declared
   * `{name}` in the source must be bound (an unused param, or a placeholder
   * with no matching param, is a build-time error).
   *
   * @example
   * ```ts
   * const phase = port.input.text({ id: "phase" });
   * const draft = slot.text("draft");
   * const workSummary = slot.text("work-summary");
   * prompt.template(
   *   "Review the implemented work for {phase} against the draft {draft}. Current work summary: {workSummary}.",
   *   { phase, draft, workSummary },
   * );
   * ```
   */
  template<const Source extends string, const Params extends TemplateParams<Source>>(
    source: Source,
    params: ExactTemplateParams<Source, Params>,
  ): PromptTemplate<HandleValue<Params[keyof Params]>>;
  template<Params extends Record<string, PromptInterpolation>>(
    source: PromptTemplate | ResourceHandle,
    params: Params,
  ): PromptTemplate<HandleValue<Params[keyof Params]>>;
}

/**
 * Declares prompts and typed prompt templates: the text sent to an agent,
 * with interpolated ports/slots/resources tracked as typed inputs instead
 * of raw string concatenation.
 *
 * A hand-written prompt body is an opaque string — the runtime cannot tell
 * which slots or ports it actually reads, so it can neither validate nor
 * inject them correctly. Building the text with `prompt.text`/
 * `prompt.template` (or passing one to `sequence.prompt`'s `text` field
 * directly, which is the common path — declaring a standalone named
 * `prompt(...)` is only needed when the same prompt is reused across steps)
 * means every interpolation is a typed reference, and a stale reference to a
 * renamed slot fails at `tsc`.
 *
 * @param fields Prompt declaration fields.
 * @example
 * ```ts
 * const request = port.input.text({ id: "request" });
 * prompt.text`Summarize ${request}`;
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

// `text`/`template` stay generic interface members compared against plain
// (non-generic) implementations here — the same `Object.assign`
// generic-merge limitation `port.ts`/`slot.ts` document — so the namespace
// as a whole still binds through one cast; `promptFn` itself (the part that
// used to read `args[n]`) is now a real checked overloaded function.
export const prompt = Object.assign(
  promptFn,
  {
    text: (strings: TemplateStringsArray, ...values: readonly PromptInterpolation[]): PromptTemplate =>
      promptTemplate(strings, values),
    template: (
      source: string | PromptTemplate | ResourceHandle,
      params: Record<string, PromptInterpolation>,
    ): PromptTemplate => promptNamedTemplate(source, params),
    resource: (
      value: ResourceHandle | RefHandle | {
        readonly resource: ResourceHandle | RefHandle | string;
        readonly input?: readonly unknown[];
      },
    ): PromptTemplate => promptResource(value),
  },
) as PromptFunction;
function promptNamedTemplate(
  source: string | PromptTemplate | ResourceHandle,
  params: Record<string, PromptInterpolation>,
): PromptTemplate {
  recordDiagnostic(
    "cdk-prompt-template",
    "prompt.template",
    "prompt.template(...) is deprecated; use prompt.text`...` or a named prompt declaration",
  );
  const sourceText = typeof source === "string" ? source : source.text;
  if (typeof sourceText === "string") {
    const { text, refs, optionalRefs } = rewriteNamedTemplate(sourceText, params);
    return withMeta({ text }, {
      kind: "template",
      refs,
      optionalRefs,
      declaration: { text },
      declarations: collectMany([source, ...Object.values(params)]),
    });
  }

  const sourceRef = promptSourceValue(source, "prompt.template.source");
  if (typeof sourceRef !== "string") {
    throw new Error("prompt.template: prompt templates must contain text");
  }
  const declarations = collectMany([source, ...Object.values(params)]);
  const resourceId = sourceRef.slice("resource:".length);
  const resourceDeclaration = declarations.resource?.find((declaration) => declaration.id === resourceId);
  const content = resourceDeclaration?.content;
  if (typeof content === "string") {
    const rewritten = rewriteNamedTemplate(content, params);
    return withMeta({ source: sourceRef, input: rewritten.refs }, {
      kind: "template",
      refs: unique([sourceRef, ...rewritten.refs]),
      declaration: { source: sourceRef },
      declarations: {
        ...declarations,
        resource: declarations.resource?.map((declaration) =>
          declaration.id === resourceId
            ? { ...declaration, content: rewritten.text, input: rewritten.refs }
            : declaration
        ) ?? [],
      },
    });
  }
  const refs = Object.values(params).map((value, index) =>
    requirePromptRef(value, `prompt.template parameter ${index}`)
  );
  return withMeta({ source: sourceRef, input: refs }, {
    kind: "template",
    refs: unique([sourceRef, ...refs]),
    declaration: { source: sourceRef },
    declarations: {
      ...declarations,
      resource: declarations.resource?.map((declaration) =>
        declaration.id === resourceId ? { ...declaration, input: refs } : declaration
      ) ?? [],
    },
  });
}

function rewriteNamedTemplate(
  source: string,
  params: Record<string, PromptInterpolation>,
): { text: string; refs: string[]; optionalRefs: string[]; } {
  const refs: string[] = [];
  const optionalRefs: string[] = [];
  const text = source.replace(/\{([^{}]+)\}/g, (token, name: string) => {
    if (REF_TOKEN_PATTERN.test(name)) {
      refs.push(name);
      return token;
    }
    if (!/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name)) {
      return token;
    }
    const value = params[name];
    if (value === undefined) throw new Error(`prompt.template: missing parameter ${JSON.stringify(name)}`);
    const resolved = resolvePromptRef(value, `prompt.template parameter ${JSON.stringify(name)}`);
    refs.push(resolved.ref);
    if (resolved.optional) optionalRefs.push(resolved.ref);
    return `{${resolved.ref}}`;
  });
  for (const key of Object.keys(params)) {
    if (!new RegExp(`\\{${escapeRegExp(key)}\\}`).test(source)) {
      throw new Error(`prompt.template: unused parameter ${JSON.stringify(key)}`);
    }
  }
  return { text, refs: uniqueInOrder(refs), optionalRefs: uniqueInOrder(optionalRefs) };
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

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

function promptTemplate(strings: TemplateStringsArray, values: readonly PromptInterpolation[]): PromptTemplate {
  let text = strings[0] ?? "";
  const refs: string[] = [];
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (value === undefined) throw new Error(`prompt.text interpolation ${index}: expected a reference`);
    const ref = requirePromptRef(value, `prompt.text interpolation ${index}`);
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
