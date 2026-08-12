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
import { settingMetaOf } from "@ctx-traits/config";
import type { FlattenedVariant, SchemaValue, VariantTopologyNode } from "./authoring-types.js";
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
import type { Brand, SequenceHandle, SlotHandle, WithHandleValue } from "./handles.js";

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
  readonly "output-sink": { readonly slot: string; readonly operation: WriteOperation };
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

// sdk-generate validates this literal union from the source text.
// prettier-ignore
export type DeclKind = "agent" | "condition" | "port" | "prompt" | "resource" | "schema" | "sequence" | "session" | "setting" | "signal" | "slot";

export interface Meta {
  // sdk-generate validates this literal union from the source text.
  // prettier-ignore
  readonly kind?: DeclKind | "trait" | "procedure" | "sequence-step" | "sequence-linear" | "template" | "schema-field" | "guard" | "ref" | "behavior" | "output-sink" | "instruction-output" | "output-template";
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
  readonly defaultOutput?: boolean;
  readonly defaultOutputPaths?: readonly (readonly (string | number)[])[];
  readonly source?: SourceAnchor;
  readonly sourceMap?: SourceMap;
  /**
   * `output.text`/`output.of(schema)`'s own compiled instruction body
   * (`kind: "instruction-output"` only) — the text plus, for `.of`, the
   * schema ref, rendered into the attaching step's prompt via
   * `output.ts`'s `OUTPUT_RENDER_V1`. Immutable once built; `ref` (above)
   * is what mutates at attach time.
   */
  readonly instructionOutput?: {
    readonly text: string;
    readonly schemaRef?: string;
    /** Refs the instruction text itself interpolated (e.g. `output.text`Summarize ${diff}.`` ),
     * so the attaching step's implicit prompt inherits them as ordinary inputs. */
    readonly refs?: readonly string[];
    readonly optionalRefs?: readonly string[];
  };
  /**
   * `output.prompt`'s own compiled instruction body (`kind:
   * "output-template"` only): the text plus each interpolated slot's ref
   * (and its resolved value, so the attaching step can validate authorship
   * against the actual slot handle rather than only its ref string) —
   * `refs` IS the step's output contract, not merely refs the text
   * interpolated.
   */
  readonly outputTemplate?: {
    readonly text: string;
    readonly refs: readonly string[];
    readonly optionalRefs?: readonly string[];
    readonly slots: readonly (SlotHandle | undefined)[];
  };
  /**
   * CDK-only provenance recording which step's `output.prompt` first
   * claimed this slot as an output-template contract, and for which agent —
   * mutated onto the interpolated slot handle's OWN meta object in place
   * (the same pattern `attachInstructionOutput` uses for its `ref`), so the
   * claim's lifetime is scoped to the slot object itself rather than a
   * module-level registry that would leak across independent trait builds
   * in the same process. A second `output.prompt` claim on the same slot
   * handle for a different agent is a build error (P105 build rule 3).
   */
  readonly outputTemplateClaim?: { readonly agent: string | undefined; readonly stepId: string };
  readonly inlineBranchArms?:
    | {
        readonly trueArm?: readonly SequenceHandle[];
        readonly falseArm?: readonly SequenceHandle[];
      }
    | undefined;
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
  readonly fieldRef?: { readonly slotRef: string; readonly field: string };
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
   * handle: the flattened, validated variant topology and variant list —
   * `variant.ts`'s `resolveTraitFamily` reads this to resolve imports and
   * assemble each variant's canonical draft. Never emitted directly.
   */
  readonly family?: {
    readonly id: string;
    readonly version: string;
    readonly topology: VariantTopologyNode;
    readonly variants: readonly FlattenedVariant[];
  };
}

export const META: unique symbol = Symbol("ctx.traits.cdk.meta");
const CDK_SOURCE_PATH = decodeURIComponent(new URL(import.meta.url).pathname);
const authoredDeclarations: { readonly kind: DeclKind; readonly ref: string; readonly declaration: JsonObject }[] = [];
const processDiagnostics: CdkDiagnostic[] = [];

export type CdkObject = { readonly [key: string]: unknown; readonly [META]?: Meta };

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
>(value: T, meta: Meta & { readonly kind?: MetaKind }): WithHandleValue<T & Brand<HandleKind>, Value> {
  const anchoredMeta = meta.source === undefined ? { ...meta, source: captureSourceAnchor() } : meta;
  Object.defineProperty(value, META, { value: anchoredMeta, enumerable: false, configurable: true });
  return value as WithHandleValue<T & Brand<HandleKind>, Value>;
}

/**
 * A `setting.*` handle (minted by `@ctx-traits/config`, 0180) carries no
 * CDK `META` symbol — it synthesizes a `Meta` from the handle's config-side
 * interop meta instead. `metaOf` is the single choke point behind
 * `explicitDeclarationItems` (normalize.ts), the `merged.setting` merge
 * (trait.ts), and every `setting:` ref-prefix check (sequence.ts), so this
 * one fallback covers prompt interpolation, argv tokens, condition
 * operands, and structural fields like `loop.maxIterations` alike.
 */
export function metaOf(value: unknown): Meta | undefined {
  const meta = typeof value === "object" && value !== null ? (value as { readonly [META]?: Meta })[META] : undefined;
  if (meta !== undefined) return meta;
  const settingMeta = settingMetaOf(value);
  if (settingMeta === undefined) return undefined;
  return {
    kind: "setting",
    ref: settingMeta.ref,
    declaration: settingMeta.declaration as unknown as MetaDeclaration, // audited-unknown-cast: config's SettingDeclaration mirrors CanonicalSetting field-for-field
  };
}

/** `value`'s `output-sink` declaration, narrowed by `Meta<K>`'s kind check — shared by every output-ref reader (sequence.ts, condition.ts). */
export function outputSinkDeclaration(value: unknown): CanonicalDeclarationByKind["output-sink"] | undefined {
  const meta = metaOf(value);
  return meta?.kind === "output-sink" ? (meta.declaration as CanonicalDeclarationByKind["output-sink"]) : undefined;
}

/**
 * Resolves an `output.text`/`output.of(...)` instruction-output handle to
 * its attaching step's auto-declared slot: sets `ref` and folds the slot's
 * own declaration into `declarations.slot`, mutating the handle's OWN meta
 * object in place (not copying it) so any interpolation of the same bound
 * const anywhere else in the trait source — before or after this call, in
 * authoring order — reads the same resolved value the next time it's read.
 * `sequence.ts`'s `output:` normalization calls this exactly once per
 * instruction-output handle; a second call throws (an instruction-output can
 * back only one slot).
 */
export function attachInstructionOutput(value: unknown, slotRef: string, slotDeclaration: JsonObject): void {
  const meta = metaOf(value) as (Meta & { ref?: string }) | undefined;
  if (meta?.kind !== "instruction-output") {
    throw new Error("attachInstructionOutput: expected an output.text/output.of instruction-output handle");
  }
  if (meta.ref !== undefined) {
    throw new Error(`output.text/output.of: already attached to ${meta.ref} — cannot also attach it to ${slotRef}`);
  }
  (meta as { ref?: string }).ref = slotRef;
  (meta as { declarations?: Partial<Record<DeclKind, readonly JsonObject[]>> }).declarations = {
    ...meta.declarations,
    slot: [...(meta.declarations?.slot ?? []), slotDeclaration],
  };
}

/** Narrows to an `output.text`/`output.of(...)` instruction-output handle. */
export function isInstructionOutputHandle(value: unknown): boolean {
  return metaOf(value)?.kind === "instruction-output";
}

/** The compiled instruction text (+ optional schema ref) an instruction-output handle carries, or `undefined` if it isn't one. */
export function instructionOutputContent(value: unknown):
  | {
      readonly text: string;
      readonly schemaRef?: string;
      readonly refs?: readonly string[];
      readonly optionalRefs?: readonly string[];
    }
  | undefined {
  const meta = metaOf(value);
  return meta?.kind === "instruction-output" ? meta.instructionOutput : undefined;
}

/** Narrows to an `output.prompt` output-template handle. */
export function isOutputTemplateHandle(value: unknown): boolean {
  return metaOf(value)?.kind === "output-template";
}

/** The compiled instruction text + interpolated slot refs an `output.prompt` handle carries, or `undefined` if it isn't one. */
export function outputTemplateContent(value: unknown): Meta["outputTemplate"] {
  const meta = metaOf(value);
  return meta?.kind === "output-template" ? meta.outputTemplate : undefined;
}

/**
 * Claims `slotValue` (an output.prompt interpolation site's resolved slot
 * handle) as this step's output-template contract. Throws when the same
 * slot handle was already claimed by a different step for a different
 * agent — see {@link Meta.outputTemplateClaim}. A non-handle interpolation
 * (a bare ref string) carries no meta to mutate and is silently skipped:
 * authorship tracking degrades gracefully rather than blocking the author
 * from writing `output.prompt` with a bare `"slot:x"` ref.
 */
export function claimOutputTemplateAuthorship(slotValue: unknown, agent: string | undefined, stepId: string): void {
  const meta = metaOf(slotValue) as
    | (Meta & { outputTemplateClaim?: { agent: string | undefined; stepId: string } })
    | undefined;
  if (meta === undefined) return;
  const existing = meta.outputTemplateClaim;
  if (existing !== undefined && existing.agent !== agent) {
    throw new Error(
      `procedure.sequence ${stepId}: output.prompt claims ${meta.ref ?? "this output slot"} for agent ` +
        `${agent ?? "(none)"}, but procedure.sequence ${existing.stepId} already claims it for agent ` +
        `${existing.agent ?? "(none)"} — an output-template contract belongs to exactly one agent`,
    );
  }
  (meta as { outputTemplateClaim?: { agent: string | undefined; stepId: string } }).outputTemplateClaim = {
    agent,
    stepId,
  };
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
  const handle = withMeta<T, MetaKind, Value, HandleKind>(publicSurface, {
    ...extraMeta,
    kind,
    ref,
    // `declaration` is assembled dynamically per call site (compact(...)
    // over a per-kind field record); the type's payoff is at the reader
    // boundary (`Meta.declaration: MetaDeclaration`), not this single
    // producer — same reasoning as `assembleSingleTraitDraft`'s draft cast
    // (P481 §4.5). This is the one audited `declaration` cast every
    // declaration builder routes through (P485 Phase 2) — callers no
    // longer assert their own `Handle<K, Value>` shape at each call site.
    declaration: declaration as unknown as MetaDeclaration, // audited-unknown-cast: the one declaration-producing cast every builder routes through, see comment above
  } as Meta & { readonly kind?: MetaKind });
  attachMeta(declaration, metaOf(handle) as Meta);
  if (isDeclarationKind(kind)) authoredDeclarations.push({ kind, ref, declaration });
  return handle;
}

/** Records a build-only authoring advisory without changing emitted values. */
export function recordDiagnostic(code: string, fieldPath: string, message: string, source?: SourceAnchor): void {
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
  return [
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
  ].includes(value);
}

/**
 * Attaches non-enumerable CDK metadata without capturing a source anchor —
 * for internal composition values (a spread-composed object-schema field
 * entry, for instance) that are not themselves a top-level authored
 * declaration and don't need their own authoring location recorded.
 * `T extends object` (not `CdkObject`) deliberately: `Object.defineProperty`
 * needs no index signature, and this lets closed canonical shapes (no index
 * signature of their own, e.g. `SchemaFieldRecord`/`GuardHandle`) round-trip
 * through here without a `CdkObject` detour cast at every call site.
 */
export function attachMeta<T extends object>(value: T, meta: Meta): T {
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
): T & { readonly [K in Field]: Value } {
  Object.defineProperty(value, field, { value: fieldValue, enumerable: false });
  return value as T & { readonly [K in Field]: Value };
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

function parseStackFrame(line: string): { readonly file: string; readonly line: number } | undefined {
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
  return file === CDK_SOURCE_PATH || file.includes("/packages/cdk/src/") || file.includes("/packages/cdk/dist/");
}
