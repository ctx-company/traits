export type {
  CdkDiagnostic,
  JsonObject,
  JsonValue,
  RefKind,
  RuntimeDelegationPlan,
  RuntimeOperation,
  RuntimeSurface,
  SynthFormat,
} from "./generated.js";

import type { CanonicalTraitDraft, CdkDiagnostic, JsonObject, JsonValue } from "./generated.js";
import type {
  RuntimeDelegationPlan,
  RuntimeSurface,
  SynthFormat,
  SynthPlan as GeneratedSynthPlan,
} from "./generated.js";
import type { Brand } from "./handles.js";
import type { CanonicalDeclarationByKind } from "./meta.js";
import {
  authoredDeclarationRecords,
  metaOf,
  processAuthoringDiagnostics,
  resetAuthoringState,
  withMeta,
} from "./meta.js";
import type { DeclKind, SourceAnchor, SourceMap } from "./meta.js";
import type { BehaviorFields, IntentSpec, TraitFields } from "./trait.js";

/**
 * `toDraftJson`'s accepted input: any branded CDK handle (not just
 * `TraitHandle` — `toDraftJson` is genuinely called on agent/session/other
 * declaration handles too, e.g. cdk.test.ts's session tests) or an
 * already-plain draft `JsonObject`. Widening past `TraitHandle | JsonObject`
 * is the one named borrow from P484 item (2): P481's own Done-when (zero
 * `toDraftJson(...) as {` casts in the test suite) is unreachable without it.
 */
export type Draftable = Brand<string> | JsonObject;

/**
 * The canonical draft type a handle lowers to under `toDraftJson`: a `trait`
 * handle yields `CanonicalTraitDraft`, any other branded declaration handle
 * yields its own canonical declaration shape, and a plain (unbranded)
 * `JsonObject` passes through unchanged.
 */
export type DraftOf<T> = T extends Brand<"trait"> ? CanonicalTraitDraft
  : T extends Brand<infer K> ? (K extends keyof CanonicalDeclarationByKind ? CanonicalDeclarationByKind[K] : JsonObject)
  : JsonObject;

/**
 * `SynthPlan` narrowed to the canonical draft shape: the generated
 * `SynthPlan.draft: JsonObject` stays the wire-level contract other
 * consumers (WASM/CLI) see, while `synth(...)`'s own return type carries the
 * precise draft shape for the handle it was called with.
 */
export type SynthPlan<T = Draftable> = Omit<GeneratedSynthPlan, "draft"> & {
  readonly draft: DraftOf<T>;
};

const declKinds: readonly DeclKind[] = [
  "agent",
  "condition",
  "port",
  "prompt",
  "resource",
  "schema",
  "sequence",
  "session",
  "signal",
  "slot",
];

/**
 * Lowers a `TraitHandle` (or already-plain draft JSON) to the stable,
 * deterministically key-sorted JSON object the Rust core consumes — the
 * final step between a CDK trait source and `ctx traits synth`/`build`.
 *
 * Handles carry CDK-only bookkeeping (declaration order, source anchors,
 * collected-but-not-yet-flattened declarations) that must never reach the
 * emitted document; `toDraftJson` strips all of that and produces the same
 * byte-for-byte JSON regardless of the order fields were declared in code,
 * which is what makes the emitted draft diffable and reproducible across
 * builds.
 *
 * @example `const draft = toDraftJson(trait("review", { behavior: { tone: tone("blunt") } }));`
 * @see {@link synth}
 */
export function toDraftJson<T extends Draftable>(value: T): DraftOf<T> {
  return stableObject(normalizeValue(value) as JsonObject) as DraftOf<T>;
}

/**
 * Like {@link toDraftJson}, but also returns `__map`: a source map from
 * every emitted ref (`schema:foo`, `sequence:bar`, ...) back to the trait
 * source location that declared it — what a build error or trust-review
 * tool uses to point at the authored line instead of only the emitted JSON.
 * @example `const { draft, __map } = toDraftJsonWithSourceMap(trait("review", { behavior: { tone: tone("blunt") } }));`
 */
export function toDraftJsonWithSourceMap<T extends Draftable>(
  value: T,
): {
  readonly draft: DraftOf<T>;
  readonly __map: SourceMap;
  readonly diagnostics: readonly CdkDiagnostic[];
  readonly authoredDeclarations: readonly {
    readonly kind: DeclKind;
    readonly ref: string;
    readonly declaration: JsonObject;
    readonly source?: SourceAnchor;
  }[];
} {
  const draft = toDraftJson(value);
  const __map = sourceMapFor(value, draft);
  const authoredDeclarations = authoredDeclarationRecords();
  const diagnostics = uniqueDiagnostics(processAuthoringDiagnostics());
  resetAuthoringState();
  return { draft, __map, diagnostics, authoredDeclarations };
}

/**
 * Every handle-tree walker below (`diagnostics`, `collectMany`,
 * `collectDiagnostics`, `collectSourceMaps`, `sourceMapFor`) recurses into a
 * CDK object's own enumerable member values to reach nested handles —
 * `Object.values` typed against `object` (not `Record<string, unknown>`)
 * already returns `unknown[]`, so this needs no cast; it exists once so the
 * five walkers don't each re-derive it.
 */
function memberValues(value: object): readonly unknown[] {
  return Object.values(value);
}

/**
 * Collects every CDK-side diagnostic (deprecation notices, authoring
 * warnings) attached anywhere within a handle tree, de-duplicated and
 * stably sorted — the CDK-level counterpart to the Rust validator's
 * diagnostics, surfaced before a draft ever reaches synth.
 * @example `const problems = diagnostics(trait("review", { behavior: { tone: tone("blunt") } }));`
 */
export function diagnostics(value: unknown): readonly CdkDiagnostic[] {
  const collected: CdkDiagnostic[] = [];
  const visit = (item: unknown): void => {
    if (item === undefined || item === null) return;
    if (Array.isArray(item)) return void item.forEach(visit);
    if (typeof item !== "object") return;
    const meta = metaOf(item);
    collected.push(...(meta?.diagnostics ?? []));
    if (meta?.declarations === undefined) memberValues(item).forEach(visit);
  };
  visit(value);
  return uniqueDiagnostics(collected);
}

/**
 * Combines {@link toDraftJson} and {@link diagnostics} into one call: the
 * common shape a CLI or build tool wants (the draft to write, plus whatever
 * CDK-level problems to report) without computing the same handle tree twice.
 * @example `const { draft, diagnostics } = toDraftJsonWithDiagnostics(trait("review", { behavior: { tone: tone("blunt") } }));`
 */
export function toDraftJsonWithDiagnostics<T extends Draftable>(
  value: T,
): { readonly draft: DraftOf<T>; readonly diagnostics: readonly CdkDiagnostic[]; } {
  return { draft: toDraftJson(value), diagnostics: diagnostics(value) };
}

/**
 * Builds a `SynthPlan`: the draft JSON plus the exact delegated command
 * (`ctx traits synth <draft.json> --format <format>`) that turns it into the
 * canonical TOML trait document. The CDK never re-implements synth itself —
 * every canonical-document rule (slug validation, dependency resolution,
 * `[schema-version]` semantics, ...) lives once in the Rust core, and
 * `synth(...)` is the typed description of the call that reaches it,
 * ready for a host to execute via WASM or the CLI.
 * @example `const plan = synth(trait("review", { behavior: { tone: tone("blunt") } }), { format: "toml" });`
 * @see {@link toDraftJson}
 */
export function synth<T extends Draftable>(
  draft: T,
  options: { readonly format?: SynthFormat; readonly out?: string; } = {},
): SynthPlan<T> {
  const format = options.format ?? "toml";
  const command = ["ctx", "traits", "synth", "<draft.json>", "--format", format];
  if (options.out !== undefined) command.push("--out", options.out);
  return {
    delegate: "ctx-traits-wasm-core-or-cli",
    command,
    draft: toDraftJson(draft),
    diagnostics: diagnostics(draft),
    format,
    ...(options.out === undefined ? {} : { out: options.out }),
  };
}

/**
 * Builds a `RuntimeDelegationPlan`: the typed description of a runtime
 * operation (other than synth) that must delegate to the Rust core —
 * naming the WASM export or CLI command a host should call and the request
 * payload to call it with, rather than the CDK re-implementing runtime
 * behavior in TypeScript.
 * @param operation The runtime operation to delegate, e.g. `"validate"`.
 * @param request The operation's request payload.
 * @example
 * ```ts
 * const draft = toDraftJson(trait("review", { behavior: { tone: tone("blunt") } }));
 * runtimeDelegation("validate", { draft }, "wasm-core");
 * ```
 * @see {@link synth}
 */
export function runtimeDelegation(
  operation: RuntimeDelegationPlan["operation"],
  request: JsonValue,
  surface: RuntimeSurface = "wasm-core",
): RuntimeDelegationPlan {
  const operationName = operation.replaceAll("-", "_");
  return compactAs<RuntimeDelegationPlan>({
    surface,
    operation,
    ...(surface === "cli-json" ? {} : { wasmExport: `${operationName}_json` }),
    ...(surface === "cli-json" ? { cliCommand: ["ctx", "traits", operation, "--json"] } : {}),
    request,
    note: "delegates-to-rust-core" as const,
  });
}

export function normalizeValue(value: unknown): JsonValue | undefined {
  if (value === undefined) return undefined;
  const meta = metaOf(value);
  if (meta?.ref !== undefined && meta.declaration === undefined && meta.kind === "ref") return meta.ref;
  // A declared handle's own enumerable surface can differ from its
  // canonical declaration (an object schema handle's own keys are its
  // normalized field records, for JavaScript-spread composition — see
  // schema.ts); `meta.declaration` is always the authoritative JSON to
  // emit for a declared item, so prefer it over the value's own properties
  // whenever both exist.
  const target = meta?.declaration ?? value;
  if (Array.isArray(target)) return target.map(normalizeValue).filter((item) => item !== undefined) as JsonValue[];
  if (typeof target !== "object" || target === null) return target as JsonValue;
  return stableObject(
    Object.fromEntries(
      Object.entries(target as JsonObject).filter(([, item]) => item !== undefined).map(([key, item]) =>
        [key, normalizeValue(item)] as const
      ).filter(([, item]) => item !== undefined),
    ) as JsonObject,
  );
}

export function normalizeRefList(value: unknown): string[] | undefined {
  if (value === undefined) return undefined;
  const values = Array.isArray(value) ? value : [value];
  return values.map((item, index) => refText(item, `ref list item ${index}`));
}

export function collectMany(values: readonly unknown[]): Partial<Record<DeclKind, readonly JsonObject[]>> {
  const result: Record<DeclKind, JsonObject[]> = {
    agent: [],
    condition: [],
    port: [],
    prompt: [],
    resource: [],
    schema: [],
    sequence: [],
    session: [],
    signal: [],
    slot: [],
  };
  const visited = new Set<object>();
  const visit = (value: unknown): void => {
    if (value === undefined || value === null) return;
    if (Array.isArray(value)) return void value.forEach(visit);
    if (typeof value !== "object") return;
    if (visited.has(value)) return;
    visited.add(value);
    const meta = metaOf(value);
    if (meta?.declaration !== undefined && isDeclKind(meta.kind)) {
      result[meta.kind].push(meta.declaration);
      visit(meta.declaration);
    }
    for (const kind of declKinds) {
      for (const declaration of meta?.declarations?.[kind] ?? []) {
        result[kind].push(declaration);
        visit(declaration);
      }
    }
    memberValues(value).forEach(visit);
  };
  values.forEach(visit);
  return result;
}

export function collectDiagnostics(values: readonly unknown[]): readonly CdkDiagnostic[] {
  const result: CdkDiagnostic[] = [];
  const visit = (value: unknown): void => {
    if (value === undefined || value === null) return;
    if (Array.isArray(value)) return void value.forEach(visit);
    if (typeof value !== "object") return;
    result.push(...(metaOf(value)?.diagnostics ?? []));
    memberValues(value).forEach(visit);
  };
  values.forEach(visit);
  return uniqueDiagnostics(result);
}

export function collectSourceMaps(values: readonly unknown[]): SourceMap {
  const entries: [string, SourceAnchor][] = [];
  const visit = (value: unknown): void => {
    if (value === undefined || value === null) return;
    if (Array.isArray(value)) return void value.forEach(visit);
    if (typeof value !== "object") return;
    entries.push(...Object.entries(metaOf(value)?.sourceMap ?? {}));
    memberValues(value).forEach(visit);
  };
  values.forEach(visit);
  return Object.fromEntries(entries.sort(([a], [b]) => a.localeCompare(b)));
}

export function sourceMapForSequenceItems(
  containerId: string,
  sourceItems: readonly unknown[],
  normalizedItems: readonly JsonObject[],
): SourceMap {
  const entries: [string, SourceAnchor][] = [];
  for (let index = 0; index < normalizedItems.length; index += 1) {
    const anchor = metaOf(sourceItems[index])?.source;
    const item = normalizedItems[index];
    if (anchor !== undefined) {
      entries.push([`sequence:${containerId}/${typeof item?.id === "string" ? item.id : `step-${index + 1}`}`, anchor]);
    }
  }
  return Object.fromEntries(entries.sort(([a], [b]) => a.localeCompare(b)));
}

/** The shared lowercase-slug shape: a custom behavior/intent slug, a trait id, and a variant path segment. */
const SLUG_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** Whether `value` is a lowercase, hyphen-separated slug (`foo`, `foo-bar`, not `foo_bar`/`Foo`/`""`). */
export function isSlug(value: string): boolean {
  return SLUG_PATTERN.test(value);
}

export function compact(value: Record<string, unknown>): JsonObject {
  return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined)) as JsonObject;
}
export function stableObject<T extends JsonObject>(value: T): T {
  return Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined).sort(([a], [b]) => a.localeCompare(b)).map((
      [key, item],
    ) => [key, stableJson(item as JsonValue)]),
  ) as T;
}

/** Mutable-in, canonical-readonly-out counterpart to {@link compact}, for producers (e.g. `sequence.ts`) that build a canonical type's body via incremental key writes on a `Mutable<T>` before returning the readonly `T`. */
export function compactAs<T>(value: Mutable<T>): T {
  return compact(value as Record<string, unknown>) as unknown as T; // audited-unknown-cast: compactAs<T> is the one generic producer-side mint every compactAs caller routes through
}

/** `-readonly` counterpart to a canonical document type, for incremental construction. */
export type Mutable<T> = { -readonly [K in keyof T]: T[K]; };
export function mutableJsonArray(values: readonly JsonObject[] | undefined): JsonObject[] | undefined {
  return values === undefined ? undefined : [...values];
}
export function mutableScalarArray(value: string | readonly string[] | undefined): string | string[] | undefined {
  return value === undefined || typeof value === "string" ? value : Array.from(value);
}
/** Renders a level-two heading over an already-formatted body, shared by every doc-resource helper's Markdown output. */
export function headingBlock(title: string, body: string): string {
  return body === "" ? `## ${title}\n` : `## ${title}\n\n${body}\n`;
}
export function unique(values: readonly string[]): string[] {
  return [...new Set(values)].sort();
}
export function uniqueInOrder(values: readonly string[]): string[] {
  return [...new Set(values)];
}
/** Canonical grammar for a `{port:x}` / `{slot:x}` / `{resource:x}` interpolation token, shared by prompt and command interpolation. */
export const REF_TOKEN_PATTERN = /^(?:port|slot|resource):[A-Za-z0-9][A-Za-z0-9/-]*$/;
const SHELL_OPERATOR_CHARS = new Set(["|", "&", ";", "<", ">", "`", "$", "(", ")", "{", "}", "*", "?", "~"]);
const SHELL_WHITESPACE = /\s/;
/**
 * Splits a literal (non-interpolated) command segment into argv words using a
 * deterministic shell-word tokenizer: single- and double-quoted spans are
 * taken as one word each (with `\"`, `\\`, and `` \` `` escapes recognized
 * inside double quotes, and a bare backslash escaping the next character
 * outside quotes), while pipes, redirects, substitutions, and other
 * shell-execution operators are rejected everywhere — no shell is ever
 * invoked, so this never needs to parse those constructs, only refuse them.
 * Shared by `sequence.command`'s `cmd` shorthand and `input.command`'s
 * tagged-template literal segments.
 */
export function tokenizeShellLiteral(segment: string, path: string): string[] {
  if (/\n|\r/.test(segment)) {
    throw new Error(`${path}: command shorthand contains shell-only syntax; use explicit argv`);
  }
  const fail = (): never => {
    throw new Error(`${path}: command shorthand contains shell-only syntax; use explicit argv`);
  };
  const words: string[] = [];
  let current = "";
  let inWord = false;
  let i = 0;
  while (i < segment.length) {
    const ch = segment.charAt(i);
    if (SHELL_WHITESPACE.test(ch)) {
      if (inWord) {
        words.push(current);
        current = "";
        inWord = false;
      }
      i++;
      continue;
    }
    if (ch === "'") {
      const end = segment.indexOf("'", i + 1);
      if (end === -1) fail();
      current += segment.slice(i + 1, end);
      inWord = true;
      i = end + 1;
      continue;
    }
    if (ch === "\"") {
      let j = i + 1;
      let literal = "";
      for (;;) {
        if (j >= segment.length) fail();
        const c = segment.charAt(j);
        if (c === "\"") break;
        const next = segment.charAt(j + 1);
        if (c === "\\" && j + 1 < segment.length && (next === "\"" || next === "\\" || next === "`" || next === "$")) {
          literal += next;
          j += 2;
          continue;
        }
        if (c === "$" || c === "`") fail();
        literal += c;
        j++;
      }
      current += literal;
      inWord = true;
      i = j + 1;
      continue;
    }
    if (ch === "\\") {
      if (i + 1 >= segment.length) fail();
      current += segment.charAt(i + 1);
      inWord = true;
      i += 2;
      continue;
    }
    if (SHELL_OPERATOR_CHARS.has(ch) || ch === "[" || ch === "]") fail();
    current += ch;
    inWord = true;
    i++;
  }
  if (inWord) words.push(current);
  return words;
}
export function validateSlug(value: string, fieldPath: string): void {
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(value)) {
    throw new Error(`${fieldPath}: expected lowercase slug, got ${JSON.stringify(value)}`);
  }
}
export function refText(value: unknown, fieldPath: string): string {
  const ref = metaOf(value)?.ref;
  if (ref !== undefined) return ref;
  if (typeof value === "string") return value;
  throw new Error(`${fieldPath}: expected typed ref handle or ref string`);
}

export function normalizeBehavior(value: BehaviorFields | undefined): JsonObject | undefined {
  return value === undefined ? undefined : compact({
    tone: normalizeGuidanceList(value.tone),
    method: normalizeGuidanceList(value.method),
    verbosity: normalizeGuidanceItem(value.verbosity),
    directness: normalizeGuidanceItem(value.directness),
    "scope-control": normalizeGuidanceItem(value.scopeControl),
    initiative: normalizeGuidanceItem(value.initiative),
    uncertainty: normalizeGuidanceItem(value.uncertainty),
    format: normalizeGuidanceList(value.format),
  });
}
export function normalizeIntent(value: IntentSpec | undefined): JsonObject | undefined {
  return value === undefined ? undefined : compact({
    require: normalizeGuidanceList(value.require),
    focus: normalizeGuidanceList(value.focus),
    avoid: normalizeGuidanceList(value.avoid),
    block: normalizeGuidanceList(value.block),
  });
}
/** Normalizes a compact string or rich `{ id, summary?, description? }` value into stable guidance-item JSON. */
function normalizeGuidanceItem(value: unknown): JsonObject | undefined {
  if (value === undefined) return undefined;
  if (typeof value === "string") return { id: value };
  const item = value as { id?: unknown; summary?: unknown; description?: unknown; };
  return compact({ id: item.id, summary: item.summary, description: item.description });
}
/** Normalizes a scalar-or-array guidance value into an array of stable guidance-item JSON. */
function normalizeGuidanceList(value: unknown): JsonObject[] | undefined {
  if (value === undefined) return undefined;
  const items = (Array.isArray(value) ? value : [value])
    .map(normalizeGuidanceItem)
    .filter((item): item is JsonObject => item !== undefined);
  return items.length === 0 ? undefined : items;
}
/**
 * Folds each plural alias (`agents`) into its singular field (`agent`) —
 * `compact` drops the merged key entirely when both are undefined rather
 * than assigning an explicit `undefined`, which `exactOptionalPropertyTypes`
 * rejects against `TraitFields`'s non-`undefined`-typed declaration fields.
 * The cast at the boundary mirrors `assembleSingleTraitDraft`'s documented
 * producer cast (P481 §4.5): the merge is dynamic, keyed by field name, so
 * the producer side cannot be statically checked without rewriting it.
 */
export function canonicalTraitFields(fields: TraitFields): TraitFields {
  return {
    ...fields,
    ...compact({
      agent: fields.agent ?? fields.agents,
      port: fields.port ?? fields.ports,
      slot: fields.slot ?? fields.slots,
      prompt: fields.prompt ?? fields.prompts,
      sequence: fields.sequence ?? fields.sequences,
      condition: fields.condition ?? fields.conditions,
      resource: fields.resource ?? fields.resources,
      signal: fields.signal ?? fields.signals,
      session: fields.session ?? fields.sessions,
    }),
  } as TraitFields;
}
export function summaryFromTraitFields(fields: TraitFields): JsonValue | undefined {
  if (fields.summary !== undefined && fields.description !== undefined && fields.summary !== fields.description) {
    throw new Error("trait fields summary and description both supplied with different values");
  }
  return fields.summary ?? fields.description;
}
export function withoutKeys(value: Record<string, unknown>, keys: readonly string[]): JsonObject {
  const excluded = new Set(keys);
  return Object.fromEntries(Object.entries(value).filter(([key]) => !excluded.has(key))) as JsonObject;
}
// dprint-ignore: sdk-generate validates this declaration record from the source text.
export function explicitDeclarations(fields: TraitFields): Partial<Record<DeclKind, readonly JsonObject[]>> { return { agent: explicitDeclarationItems("agent", fields.agent), condition: explicitDeclarationItems("condition", fields.condition), port: explicitDeclarationItems("port", fields.port), sequence: explicitDeclarationItems("sequence", fields.sequence), slot: explicitDeclarationItems("slot", fields.slot), prompt: explicitDeclarationItems("prompt", fields.prompt), resource: explicitDeclarationItems("resource", fields.resource), signal: explicitDeclarationItems("signal", fields.signal), session: explicitDeclarationItems("session", fields.session) }; }
/**
 * Merges declaration sets and emits each kind's array in a stable order that
 * depends only on declared ids, never on authoring/registration order — see
 * `NORMALIZATION.md`. Two declarations sharing an id must be byte-identical
 * (a re-collected reference to the same declaration); anything else is a
 * genuine id collision and throws.
 */
export function mergeDeclarationSets(
  ...sets: readonly Partial<Record<DeclKind, readonly JsonObject[]>>[]
): Partial<Record<DeclKind, readonly JsonObject[]>> {
  const result: Partial<Record<DeclKind, JsonObject[]>> = {};
  for (const kind of declKinds) {
    const byId = new Map<string, JsonObject>();
    for (const set of sets) {
      for (const declaration of set[kind] ?? []) {
        const id = declaration.id;
        if (typeof id !== "string") continue;
        const meta = metaOf(declaration);
        let stable = stableObject(declaration);
        if (meta !== undefined) {
          stable = withMeta(stable, { ...meta, declaration: stable, ref: meta.ref ?? `${kind}:${id}` });
        }
        const existing = byId.get(id);
        if (existing !== undefined && JSON.stringify(existing) !== JSON.stringify(stable)) {
          const existingSource = metaOf(existing)?.source;
          const conflictingSource = meta?.source;
          const anchor = (source: SourceAnchor | undefined) =>
            source === undefined ? "unknown location" : `${source.file}:${source.start}`;
          throw new Error(
            `conflicting ${kind} declaration for id ${id} (${anchor(existingSource)} vs ${anchor(conflictingSource)})`,
          );
        }
        byId.set(id, stable);
      }
    }
    if (byId.size > 0) {
      result[kind] = [...byId.keys()].sort((a, b) => a.localeCompare(b)).map((id) => byId.get(id) as JsonObject);
    }
  }
  return result;
}
/** Renders a source anchor for a build-error message; shared by every collision error in this module. */
function anchorText(source: SourceAnchor | undefined): string {
  return source === undefined ? "unknown location" : `${source.file}:${source.start}`;
}

/**
 * Assigns stable ids to generated branch-arm/loop-gate-body sequences from
 * their deterministic base id — never a counter suffix, per `NORMALIZATION.md`.
 * A generated id colliding with another declaration (generated or authored)
 * is a build error naming both source anchors, not a silent uniquify.
 */
export function resolveGeneratedBranchArmIds(
  values: readonly unknown[],
  ...sets: readonly Partial<Record<DeclKind, readonly JsonObject[]>>[]
): void {
  const generated = new Map<JsonObject, number>();
  const bindings: JsonObject[] = [];
  const occupied = new Set<string>();
  const occupiedSource = new Map<string, SourceAnchor | undefined>();
  const seen = new Set<object>();
  const visit = (value: unknown): void => {
    if (value === undefined || value === null) return;
    if (Array.isArray(value)) return void value.forEach(visit);
    if (typeof value !== "object" || seen.has(value)) return;
    seen.add(value);
    const object = value as JsonObject;
    const meta = metaOf(object);
    if (meta?.generatedBranchArm !== undefined) {
      if (!generated.has(object)) generated.set(object, generated.size);
    }
    if (meta?.generatedBranchArmBindings !== undefined) bindings.push(object);
    for (const declarations of Object.values(meta?.declarations ?? {})) visit(declarations);
    Object.values(object).forEach(visit);
  };
  values.forEach(visit);
  sets.forEach((set) => Object.values(set).forEach(visit));
  for (const set of sets) {
    for (const declaration of set.sequence ?? []) {
      const id = declaration.id;
      if (typeof id === "string" && metaOf(declaration)?.generatedBranchArm === undefined) {
        occupied.add(id);
        occupiedSource.set(id, metaOf(declaration)?.source);
      }
    }
  }
  const allocated = new Map<JsonObject, string>();
  for (
    const [declaration] of [...generated].sort(([left, leftOrder], [right, rightOrder]) => {
      const leftBase = metaOf(left)?.generatedBranchArm?.baseId ?? "";
      const rightBase = metaOf(right)?.generatedBranchArm?.baseId ?? "";
      return leftBase.localeCompare(rightBase) || leftOrder - rightOrder;
    })
  ) {
    const generatedMeta = metaOf(declaration)?.generatedBranchArm;
    if (generatedMeta === undefined) continue;
    const id = generatedMeta.baseId;
    if (occupied.has(id)) {
      throw new Error(
        `generated sequence id ${id} collides with an existing declaration (${anchorText(occupiedSource.get(id))} vs ${
          anchorText(metaOf(declaration)?.source)
        })`,
      );
    }
    occupied.add(id);
    occupiedSource.set(id, metaOf(declaration)?.source);
    allocated.set(declaration, id);
    (declaration as { id: JsonValue; }).id = id;
    const meta = metaOf(declaration);
    if (meta !== undefined) {
      (meta as { ref?: string; }).ref = `sequence:${id}`;
      if (meta.sourceMap !== undefined) {
        const remapped = Object.fromEntries(
          Object.entries(meta.sourceMap).map(([path, anchor]) => [
            path.replace(/^sequence:[^/]+\//, `sequence:${id}/`),
            anchor,
          ]),
        );
        (meta as { sourceMap?: SourceMap; }).sourceMap = remapped;
      }
    }
  }
  for (const binding of bindings) {
    for (const [field, arm] of Object.entries(metaOf(binding)?.generatedBranchArmBindings ?? {})) {
      const declaration = (metaOf(arm)?.declaration ?? arm) as JsonObject;
      const id = allocated.get(declaration);
      if (id !== undefined) (binding as Record<string, JsonValue>)[field] = `sequence:${id}`;
    }
  }
}
export function finalizedPortDeclarations(ports: readonly JsonObject[]): JsonObject[] | undefined {
  return ports.length === 0 ? undefined : [...ports];
}
export function sequenceLinearMap(value: unknown): JsonObject | undefined {
  return declarationMap(value);
}
export function conditionMap(values: readonly unknown[]): JsonObject | undefined {
  return declarationMap(values.flatMap((value) => value === undefined ? [] : Array.isArray(value) ? value : [value]));
}
export function promptMap(value: readonly JsonObject[] | undefined): JsonObject | undefined {
  return declarationMap(value ?? []);
}

function declarationMap(value: unknown): JsonObject | undefined {
  const entries = (Array.isArray(value) ? value : value === undefined ? [] : [value]).map((item) =>
    normalizeValue(item) as JsonObject
  ).filter((item) => typeof item.id === "string").map((item) => {
    const { id, ...rest } = item as JsonObject & { id: string; };
    return [id, stableObject(rest)] as const;
  });
  return entries.length === 0 ? undefined : stableObject(Object.fromEntries(entries) as JsonObject);
}
function explicitDeclarationItems(kind: DeclKind, value: unknown): JsonObject[] {
  return (Array.isArray(value) ? value : value === undefined ? [] : [value]).map((item) =>
    declarationWithPreservedMeta(kind, item)
  );
}
function declarationWithPreservedMeta(kind: DeclKind, value: unknown): JsonObject {
  const meta = metaOf(value);
  const normalized = normalizeValue(value) as JsonObject;
  const id = typeof normalized.id === "string" ? normalized.id : undefined;
  const ref = meta?.ref ?? (id === undefined ? undefined : `${kind}:${id}`);
  if (meta === undefined && ref === undefined) return normalized;
  return withMeta(
    normalized,
    compact({
      kind,
      ref,
      declaration: normalized,
      declarations: meta?.declarations,
      refs: meta?.refs,
      diagnostics: meta?.diagnostics,
      source: meta?.source,
      sourceMap: meta?.sourceMap,
    }) as import("./meta.js").Meta,
  );
}
function isDeclKind(value: unknown): value is DeclKind {
  return typeof value === "string" && (declKinds as readonly string[]).includes(value);
}
function stableJson(value: JsonValue): JsonValue {
  return Array.isArray(value)
    ? value.map(stableJson)
    : value !== null && typeof value === "object"
    ? stableObject(value as JsonObject)
    : value;
}
function uniqueDiagnostics(values: readonly CdkDiagnostic[]): CdkDiagnostic[] {
  const byKey = new Map<string, CdkDiagnostic>();
  values.forEach((value) => byKey.set(`${value.severity}\0${value.code}\0${value.fieldPath}\0${value.message}`, value));
  return [...byKey.values()].sort((a, b) =>
    `${a.severity}:${a.code}:${a.fieldPath}:${a.message}`.localeCompare(
      `${b.severity}:${b.code}:${b.fieldPath}:${b.message}`,
    )
  );
}
/**
 * The shared finalizer between an ordinary trait and a family leaf: merges
 * every declaration's collected source map with the top-level `trait:<id>`
 * anchor from `value`'s own `Meta.source`, so both paths produce the same
 * shape of source map from the same inputs.
 */
export function sourceMapFor(value: unknown, draft: JsonObject): SourceMap {
  const entries = new Map<string, SourceAnchor>();
  const visited = new Set<object>();
  const visit = (item: unknown): void => {
    if (item === undefined || item === null) return;
    if (Array.isArray(item)) return void item.forEach(visit);
    if (typeof item !== "object") return;
    if (visited.has(item)) return;
    visited.add(item);
    const meta = metaOf(item);
    if (meta?.ref !== undefined && meta.source !== undefined) entries.set(meta.ref, meta.source);
    Object.entries(meta?.sourceMap ?? {}).forEach(([ref, anchor]) => entries.set(ref, anchor));
    for (const declarations of Object.values(meta?.declarations ?? {})) declarations.forEach(visit);
    memberValues(item).forEach(visit);
  };
  visit(value);
  if (typeof draft.id === "string" && metaOf(value)?.source !== undefined) {
    entries.set(`trait:${draft.id}`, metaOf(value)?.source as SourceAnchor);
  }
  return Object.fromEntries([...entries.entries()].sort(([a], [b]) => a.localeCompare(b)));
}
