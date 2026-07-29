export type {
  CdkDiagnostic,
  JsonObject,
  JsonValue,
  RefKind,
  RuntimeDelegationPlan,
  RuntimeOperation,
  RuntimeSurface,
  SynthFormat,
  SynthPlan,
} from "./generated.js";
import type { FlattenedVariantLeaf, SchemaValue, VariantTopologyNode } from "./authoring-types.js";
import type {
  CanonicalAgent,
  CanonicalCondition,
  CanonicalNamedSequence,
  CanonicalPort,
  CanonicalPrompt,
  CanonicalResource,
  CanonicalSchemaDeclaration,
  CanonicalSession,
  CanonicalSignal,
  CanonicalSlot,
  CanonicalTraitDraft,
  CdkDiagnostic,
  JsonObject,
  WriteOperation,
} from "./generated.js";
import type { Brand, SequenceHandle, WithHandleValue } from "./handles.js";

/**
 * Canonical JSON shape of a declaration, keyed by the CDK's handle-brand
 * strings. Shared by `Meta.declaration` and `toDraftJson`'s `DraftOf<T>`
 * (normalize.ts) so both erasure points agree on one mapping instead of two
 * ad hoc ones. Only kinds a `withDeclaration(...)` call actually produces are
 * listed — `guard`/`sequence-step`/`procedure` handles never set
 * `Meta.declaration`, only `Meta.declarations`.
 */
export type CanonicalDeclarationByKind = {
  readonly trait: CanonicalTraitDraft;
  readonly agent: CanonicalAgent;
  readonly condition: CanonicalCondition;
  readonly port: CanonicalPort;
  readonly prompt: CanonicalPrompt;
  readonly resource: CanonicalResource;
  readonly schema: CanonicalSchemaDeclaration;
  readonly sequence: CanonicalNamedSequence;
  readonly session: CanonicalSession;
  readonly signal: CanonicalSignal;
  readonly slot: CanonicalSlot;
  /** `operation.over(...)`'s CDK-only output-sink handle (slot.ts). Not a Rust-schema type — a small CDK-side wrapper, never emitted on its own (it lowers to `output` list entries). */
  readonly "output-sink": { readonly slot: string; readonly operation: WriteOperation; };
};

/**
 * The union of every declaration shape `Meta.declaration` can hold. A
 * non-generic union rather than threading `Meta<K>` through `withMeta`/
 * `withDeclaration`/every `metaOf(x)?.declaration` read site — the fallback
 * lever named in P481 risk #4, taken here to keep the ripple scoped to this
 * boundary. It still kills bare `JsonObject` at the emission boundary that
 * matters (P481's Done-when); per-kind narrowing from a generic `Meta<K>` is
 * left as a possible follow-up, not required by this phase's scope.
 */
export type MetaDeclaration = CanonicalDeclarationByKind[keyof CanonicalDeclarationByKind];

/** Source location captured when a CDK declaration is created. */
export interface SourceAnchor {
  readonly file: string;
  readonly start: number;
  readonly end: number;
}

/** Declaration references mapped to their authoring locations. */
export type SourceMap = Record<string, SourceAnchor>;

// dprint-ignore: sdk-generate validates this literal union from the source text.
export type DeclKind = "agent" | "condition" | "port" | "prompt" | "resource" | "schema" | "sequence" | "session" | "signal" | "slot";

export interface Meta {
  // dprint-ignore: sdk-generate validates this literal union from the source text.
  readonly kind?: DeclKind | "trait" | "procedure" | "sequence-step" | "sequence-linear" | "template" | "schema-field" | "guard" | "ref" | "behavior" | "output-sink";
  readonly ref?: string;
  readonly declaration?: MetaDeclaration;
  readonly declarations?: Partial<Record<DeclKind, readonly JsonObject[]>>;
  readonly refs?: readonly string[];
  /**
   * The subset of `refs` written optionally at their interpolation site
   * (`slot.optional()`). Carried here so the sequence builder can emit
   * `{ slot, optional: true }` into the step's `input` array instead of a
   * required entry — optionality is per-site, never a slot property.
   */
  readonly optionalRefs?: readonly string[];
  readonly diagnostics?: readonly CdkDiagnostic[];
  readonly order?: number;
  readonly defaultOutput?: boolean;
  readonly defaultOutputPaths?: readonly (readonly (string | number)[])[];
  readonly source?: SourceAnchor;
  readonly sourceMap?: SourceMap;
  readonly inlineBranchArms?: {
    readonly trueArm?: readonly SequenceHandle[];
    readonly falseArm?: readonly SequenceHandle[];
  } | undefined;
  /**
   * CDK-only provenance for a `loop`/`forEach` step authored with an inline
   * `body:` array (CDK grammar v2 rule 7) instead of a `sequence:` ref:
   * the un-materialized step bodies, resolved into a named `<step-id>-body`
   * sub-sequence by `materializeSequenceItem` the same way inline branch
   * arms are. Never emitted directly.
   */
  readonly inlineBody?: readonly SequenceHandle[];
  readonly generatedBranchArm?: {
    readonly baseId: string;
  };
  readonly generatedBranchArmBindings?: Partial<Record<"sequence" | "otherwise", CdkObject>>;
  /**
   * CDK-only provenance for a `sequence.branch(...)` step authored with an
   * unplaced `sequence.check(...)` step as its `check` (CDK grammar v2 rule
   * 6a): the gate step that `materializeSequenceItem` splices in as the
   * item immediately preceding this branch. Never emitted directly.
   */
  readonly precedingGateStep?: SequenceHandle;
  /** Unplaced check steps appended once after an inline loop body. */
  readonly loopGateSteps?: readonly SequenceHandle[];
  /**
   * CDK-only provenance for a `FieldRef` produced by an object-schema slot
   * proxy: the parent slot's ref text and the accessed field id. Never
   * emitted — `condition.equals` lowers it to the existing canonical
   * `{ slot, field, equals }` guard shape before normalization.
   */
  readonly fieldRef?: { readonly slotRef: string; readonly field: string; };
  /**
   * CDK-only provenance recording that a declared schema is a
   * `schema.template(...)` specialization: which template produced it and
   * the resolved schema ref for each spot. Author-only bookkeeping — never
   * reaches draft/canonical JSON.
   */
  readonly templateInfo?: {
    readonly templateId: string;
    readonly params: Readonly<Record<string, string>>;
  };
  /**
   * CDK-only provenance carried by an authored `schema.union(...)` handle:
   * its own pre-flatten member list. Lets a later `schema.union` that
   * includes this one as a member splice in its members instead of nesting
   * a union ref inside a union ref. Author-only bookkeeping — never reaches
   * draft/canonical JSON.
   */
  readonly unionMembers?: readonly SchemaValue[];
  /**
   * CDK-only provenance carried by a `trait(id, { variants })` family
   * handle: the flattened, validated variant topology and leaf list —
   * `variant.ts`'s `resolveTraitFamily` reads this to resolve imports and
   * assemble each leaf's canonical draft. Never emitted directly.
   */
  readonly family?: {
    readonly id: string;
    readonly version: string;
    readonly topology: VariantTopologyNode;
    readonly leaves: readonly FlattenedVariantLeaf[];
  };
}

export const META: unique symbol = Symbol("ctx.traits.cdk.meta");
const CDK_SOURCE_PATH = decodeURIComponent(new URL(import.meta.url).pathname);
let declarationOrder = 0;
const authoredDeclarations: { readonly kind: DeclKind; readonly ref: string; readonly declaration: JsonObject; }[] = [];
const processDiagnostics: CdkDiagnostic[] = [];

export type CdkObject = { readonly [key: string]: unknown; readonly [META]?: Meta; };

/**
 * Attach non-enumerable CDK metadata and infer the handle brand from its
 * kind. `Value` is never inferred from `value`/`meta` — a caller minting a
 * `Handle<K, Value>`-shaped return (`SchemaHandle<Value>`, `PortHandle<
 * Value>`, ...) supplies it explicitly, e.g. `withMeta<T, "port", Value>(...)`
 * — the single, centrally-justified phantom-brand mint P485 §3.4 collects
 * every per-call-site `as PortHandle<Value>`-style cast into.
 */
export function withMeta<
  T extends object,
  MetaKind extends string,
  Value = unknown,
  HandleKind extends string = MetaKind,
>(
  value: T,
  meta: Meta & { readonly kind?: MetaKind; },
): WithHandleValue<T & Brand<HandleKind>, Value> {
  const anchoredMeta = meta.source === undefined ? { ...meta, source: captureSourceAnchor() } : meta;
  Object.defineProperty(value, META, { value: anchoredMeta, enumerable: false, configurable: true });
  return value as WithHandleValue<T & Brand<HandleKind>, Value>;
}

export function metaOf(value: unknown): Meta | undefined {
  return typeof value === "object" && value !== null ? (value as { readonly [META]?: Meta; })[META] : undefined;
}

/** `value`'s `output-sink` declaration, narrowed by `Meta<K>`'s kind check — shared by every output-ref reader (sequence.ts, condition.ts). */
export function outputSinkDeclaration(value: unknown): CanonicalDeclarationByKind["output-sink"] | undefined {
  const meta = metaOf(value);
  return meta?.kind === "output-sink" ? meta.declaration as CanonicalDeclarationByKind["output-sink"] : undefined;
}

/**
 * Builds a declared-item handle: `declaration` (the canonical `{ id, ... }`
 * JSON a trait actually emits) lives only in `Meta.declaration` — what
 * `normalizeValue`/`collectMany`/`mergeDeclarationSets` read to emit,
 * collect, and dedupe declarations — while the returned handle's own
 * enumerable surface is `publicSurface`, kept separate so canonical
 * declaration internals (`id`, `schema`, ...) are never enumerable on the
 * handle authoring code holds.
 *
 * Most declaration builders (`agent`, `port`, `prompt`, checklist/resource/
 * signal/session, named `condition.all`/`any`, `sequence.linear`, declared
 * `schema.enum`, `slot`) pass an empty `publicSurface`: a declared item is
 * consumed by ref (`refText`/`metaOf`) or, for a slot, through its field
 * proxy — never by reading a declared field directly off the handle.
 * `schema.object`'s object-schema handle is the one exception: its
 * `publicSurface` is the normalized per-field record map, kept enumerable
 * so `{ ...schemaHandle }` composes with plain JavaScript spread.
 *
 * `declaration` needs its own copy of the handle's metadata (order, source
 * anchor, ref) because `collectMany`/`mergeDeclarationSets` push
 * `meta.declaration` itself — not the handle — into a trait's flattened
 * declaration lists, then read ordering/source-anchor/dedupe info back off
 * of *that* object via `metaOf`.
 *
 * Attaches metadata through the same anchored `withMeta` path `withMeta`
 * itself uses, so a declaration always carries its authored source location
 * (not just its handle) — `mergeDeclarationSets`' conflicting-id error reads
 * `meta.source` off the declaration objects it merges, and needs a real
 * anchor from both conflicting sides to name them.
 */
export function withDeclaration<
  T extends CdkObject,
  MetaKind extends string,
  Value = unknown,
  HandleKind extends string = MetaKind,
>(
  kind: MetaKind,
  ref: string,
  declaration: JsonObject,
  publicSurface: T,
  extraMeta: Meta = {},
): WithHandleValue<T & Brand<HandleKind>, Value> {
  const handle = withMeta(
    publicSurface,
    {
      order: nextDeclarationOrder(),
      ...extraMeta,
      kind,
      ref,
      // `declaration` is assembled dynamically per call site (compact(...)
      // over a per-kind field record); the type's payoff is at the reader
      // boundary (`Meta.declaration: MetaDeclaration`), not this single
      // producer — same reasoning as `assembleSingleTraitDraft`'s draft cast
      // (P481 §4.5).
      declaration: declaration as unknown as MetaDeclaration,
    } as Meta & { readonly kind?: MetaKind; },
  );
  attachMeta(declaration, metaOf(handle) as Meta);
  if (isDeclarationKind(kind)) authoredDeclarations.push({ kind, ref, declaration });
  return handle as unknown as WithHandleValue<T & Brand<HandleKind>, Value>;
}

/** Records a build-only authoring advisory without changing emitted values. */
export function recordDiagnostic(
  code: string,
  fieldPath: string,
  message: string,
  source?: SourceAnchor,
): void {
  processDiagnostics.push({
    severity: "warning",
    code,
    fieldPath,
    message: source === undefined ? message : `${message} (${source.file}:${source.start})`,
  });
}

export function processAuthoringDiagnostics(): readonly CdkDiagnostic[] {
  return processDiagnostics;
}

export function resetAuthoringState(): void {
  authoredDeclarations.length = 0;
  processDiagnostics.length = 0;
}

export function authoredDeclarationRecords(): readonly {
  readonly kind: DeclKind;
  readonly ref: string;
  readonly declaration: JsonObject;
  readonly source?: SourceAnchor;
}[] {
  return authoredDeclarations.map(({ kind, ref, declaration }) => {
    const source = metaOf(declaration)?.source;
    return source === undefined ? { kind, ref, declaration } : { kind, ref, declaration, source };
  });
}

function isDeclarationKind(value: string): value is DeclKind {
  return ["agent", "condition", "port", "prompt", "resource", "schema", "sequence", "session", "signal", "slot"]
    .includes(value);
}

/**
 * Attaches non-enumerable CDK metadata without capturing a source anchor —
 * for internal composition values (a spread-composed object-schema field
 * entry, for instance) that are not themselves a top-level authored
 * declaration and don't need their own authoring location recorded.
 */
export function attachMeta<T extends CdkObject>(value: T, meta: Meta): T {
  Object.defineProperty(value, META, { value: meta, enumerable: false, configurable: true });
  return value;
}

/**
 * Attaches a caller-facing convenience field to a handle without adding an
 * enumerable own property — the one safe way to augment a `withMeta`-built
 * handle. Plain `Object.assign(handle, { field })` is only safe on a handle
 * whose public surface is a separate lookup object from its canonical
 * declaration (`withDeclaration`-built handles, e.g. `port`/`agent`); a
 * `sequence-step`/`loop`/`sequence` handle built via `withMeta` alone IS the
 * canonical JSON `sequenceOf` returns, so an enumerable extra property would
 * serialize straight into the built canonical as an unknown field. Every
 * caller that augments such a handle (`sequence.check`'s `pass`,
 * `guardedProduction`'s `verdict`/`verdicts`) routes through this instead of
 * hand-rolling `Object.defineProperty`.
 */
export function withHiddenField<T extends object, Field extends string, Value>(
  value: T,
  field: Field,
  fieldValue: Value,
): T & { readonly [K in Field]: Value; } {
  Object.defineProperty(value, field, { value: fieldValue, enumerable: false });
  return value as T & { readonly [K in Field]: Value; };
}

export function nextDeclarationOrder(): number {
  declarationOrder += 1;
  return declarationOrder;
}

/**
 * Walks the call stack to the first frame outside `packages/cdk/src` (or
 * `dist`) — the authoring source location a CDK declaration (or a
 * `variant`/`variant.import` handle) was created from.
 */
export function captureSourceAnchor(): SourceAnchor | undefined {
  const stack = new Error().stack;
  if (stack === undefined) {
    return undefined;
  }
  for (const line of stack.split("\n").slice(1)) {
    const frame = parseStackFrame(line);
    if (frame === undefined || isCdkFrame(frame.file)) {
      continue;
    }
    return { file: frame.file, start: frame.line, end: frame.line };
  }
  return undefined;
}

function parseStackFrame(line: string): { readonly file: string; readonly line: number; } | undefined {
  const trimmed = line.trim();
  const match = /(?:at\s+(?:.*?\()?)(file:\/\/[^:)]+|\/?[^:)]+):(\d+):(\d+)\)?$/.exec(trimmed);
  if (match === null) {
    return undefined;
  }
  const rawFile = match[1] ?? "";
  const lineNumber = Number.parseInt(match[2] ?? "", 10);
  if (!Number.isFinite(lineNumber) || lineNumber <= 0) {
    return undefined;
  }
  const file = rawFile.startsWith("file://") ? decodeURIComponent(new URL(rawFile).pathname) : rawFile;
  return { file, line: lineNumber };
}

function isCdkFrame(file: string): boolean {
  return file === CDK_SOURCE_PATH
    || file.includes("/packages/cdk/src/")
    || file.includes("/packages/cdk/dist/");
}
