import {
  Directness as GeneratedDirectness,
  Format as GeneratedFormat,
  Initiative as GeneratedInitiative,
  Intent as GeneratedIntent,
  Method as GeneratedMethod,
  ScopeControl as GeneratedScopeControl,
  Tone as GeneratedTone,
  Uncertainty as GeneratedUncertainty,
  Verbosity as GeneratedVerbosity,
} from "./generated.js";
import type {
  CanonicalContract,
  CanonicalDeclaration,
  CanonicalDependency,
  CanonicalEval,
  CanonicalExtensions,
  CanonicalRelations,
  CanonicalScenario,
  CanonicalTraitDraft,
  DirectnessBuiltIn,
  FormatBuiltIn,
  InitiativeBuiltIn,
  IntentBuiltIn,
  JsonObject,
  JsonValue,
  MethodBuiltIn,
  ScopeControlBuiltIn,
  ToneBuiltIn,
  UncertaintyBuiltIn,
  VerbosityBuiltIn,
} from "./generated.js";
export type {
  DirectnessBuiltIn,
  FormatBuiltIn,
  InitiativeBuiltIn,
  IntentBuiltIn,
  MethodBuiltIn,
  ScopeControlBuiltIn,
  ToneBuiltIn,
  UncertaintyBuiltIn,
  VerbosityBuiltIn,
} from "./generated.js";
import type {
  AgentHandle,
  ConditionHandle,
  PortHandle,
  ProcedureHandle,
  PromptHandle,
  ResourceHandle,
  SequenceHandle,
  SequenceLinearHandle,
  SessionHandle,
  SettingHandle,
  SignalHandle,
  SlotHandle,
  TraitFamilyHandle,
  TraitHandle,
} from "./handles.js";
import { metaOf, withMeta } from "./meta.js";
import type { CdkDiagnostic, SourceMap } from "./meta.js";
import {
  canonicalTraitFields,
  collectDiagnostics,
  collectMany,
  collectSourceMaps,
  compact,
  conditionMap,
  descriptionFromTraitFields,
  explicitDeclarations,
  finalizedPortDeclarations,
  isSlug,
  mergeDeclarationSets,
  mutableJsonArray,
  normalizeBehavior,
  normalizeIntent,
  normalizeValue,
  promptMap,
  resolveGeneratedBranchArmIds,
  sequenceLinearMap,
  stableObject,
  summaryFromTraitFields,
  withoutKeys,
} from "./normalize.js";
import type { SinkFields } from "./sink.js";
import { readSessionTitleSink, sessionTitleSinkDraft } from "./sink.js";
import { buildTraitFamily } from "./variant.js";
import type { TraitFamilyMap } from "./variant.js";

/** A slug-identified guidance item with optional local render text. */
export interface GuidanceItem<Id extends string = string> {
  readonly id: Id;
  readonly summary?: string;
  readonly description?: string;
}
/**
 * A behavior or intent value: a built-in slug, an arbitrary custom slug, or a rich guidance item.
 * `string & {}` keeps literal auto-complete for `Id` while still accepting any string — a closed
 * union would reject the custom-slug fallback that authoring (and existing traits) rely on.
 */
export type GuidanceInput<Id extends string> = Id | (string & {}) | GuidanceItem<Id | (string & {})>;

export interface Behavior {
  readonly tone?: Tone | readonly Tone[];
  readonly method?: Method | readonly Method[];
  readonly verbosity?: Verbosity;
  readonly directness?: GuidanceInput<DirectnessBuiltIn>;
  readonly scopeControl?: GuidanceInput<ScopeControlBuiltIn>;
  readonly initiative?: GuidanceInput<InitiativeBuiltIn>;
  readonly uncertainty?: GuidanceInput<UncertaintyBuiltIn>;
  readonly format?: GuidanceInput<FormatBuiltIn> | readonly GuidanceInput<FormatBuiltIn>[];
}

/** `[intent]` authoring shape: `require`/`focus`/`avoid`/`block` guidance facets. */
/**
 * The four facets a trait sorts its intent into. Every facet accepts every
 * catalog item, because the facet is the trait's judgement about an item, not
 * a property the item carries — `avoid: [intent.ScopeCreep]` and
 * `focus: [intent.ScopeCreep]` are both meaningful and mean different things.
 */
export interface Intent {
  readonly require?: GuidanceInput<IntentBuiltIn> | readonly GuidanceInput<IntentBuiltIn>[];
  readonly focus?: GuidanceInput<IntentBuiltIn> | readonly GuidanceInput<IntentBuiltIn>[];
  readonly avoid?: GuidanceInput<IntentBuiltIn> | readonly GuidanceInput<IntentBuiltIn>[];
  readonly block?: GuidanceInput<IntentBuiltIn> | readonly GuidanceInput<IntentBuiltIn>[];
}

/**
 * `[metadata]` facets: search/browse taxonomy plus the family/variant pair
 * that groups variant packages (`<family>-<variant>`, bare `<family>` =
 * default variant). Display-only — never drives activation; the Rust synth
 * validates slugs and the family/id convention (advisory) at build time.
 */
export interface TraitMetadata {
  readonly family?: string;
  readonly variant?: string;
  readonly job?: string | readonly string[];
  readonly domain?: string | readonly string[];
  readonly artifact?: string | readonly string[];
  readonly audience?: string | readonly string[];
  readonly environment?: string | readonly string[];
  readonly tag?: string | readonly string[];
}

export interface TraitFields {
  readonly "schema-version"?: SchemaVersion;
  readonly id?: Slug;
  readonly name?: string;
  readonly version?: SemVer;
  readonly summary?: string;
  readonly description?: string;
  readonly behavior?: Behavior;
  readonly metadata?: TraitMetadata;
  readonly intent?: Intent;
  /** `[activation]` section. @see {@link rule} */
  readonly activation?: CanonicalDeclaration;
  /** `[composition]` section: conflict/merge policy against other traits. */
  readonly composition?: CanonicalContract;
  /** `[relations]` section: `conflicts`/`requires`/`suggests` against other traits. */
  readonly relations?: CanonicalRelations;
  /** `[extensions]` section: non-standard runtime extensions (`extensions.ctx.*`). */
  readonly extensions?: CanonicalExtensions;
  /** `[[dependency]]` declarations. @see {@link dependency} */
  readonly dependency?: CanonicalDependency | readonly CanonicalDependency[];
  /** The trait's procedure. @see {@link procedure} */
  readonly procedure?: ProcedureHandle;
  /** Declared agents. @example `agent: [agent.worker("impl", {...})]` @see {@link agent} */
  readonly agent?: AgentHandle | readonly AgentHandle[];
  /** Alias of {@link TraitFields.agent}. */
  readonly agents?: AgentHandle | readonly AgentHandle[];
  /** Declared ports. @example `port: [port.input.text({ id: "diff" })]` @see {@link port} */
  readonly port?: PortHandle | readonly PortHandle[];
  /** Alias of {@link TraitFields.port}. */
  readonly ports?: PortHandle | readonly PortHandle[];
  /** Declared slots. @example `slot: [slot.text("review")]` @see {@link slot} */
  readonly slot?: SlotHandle | readonly SlotHandle[];
  /** Alias of {@link TraitFields.slot}. */
  readonly slots?: SlotHandle | readonly SlotHandle[];
  /** Declared settings. @example `setting: [setting.number({...})]` @see `setting` in `@ctx-traits/config` */
  readonly setting?: SettingHandle | readonly SettingHandle[];
  /** Alias of {@link TraitFields.setting}. */
  readonly settings?: SettingHandle | readonly SettingHandle[];
  /** Declared prompts. @see {@link prompt} */
  readonly prompt?: PromptHandle | readonly PromptHandle[];
  /** Alias of {@link TraitFields.prompt}. */
  readonly prompts?: PromptHandle | readonly PromptHandle[];
  /** Declared named sequences. @see {@link sequence} */
  readonly sequence?: SequenceHandle | SequenceLinearHandle | readonly (SequenceHandle | SequenceLinearHandle)[];
  /** Alias of {@link TraitFields.sequence}. */
  readonly sequences?: SequenceHandle | SequenceLinearHandle | readonly (SequenceHandle | SequenceLinearHandle)[];
  /** Declared named conditions. @see {@link condition} */
  readonly condition?: ConditionHandle | readonly ConditionHandle[];
  /** Alias of {@link TraitFields.condition}. */
  readonly conditions?: ConditionHandle | readonly ConditionHandle[];
  /** Declared resources (`resource`/`checklist`/`rubric`). @see {@link resource} */
  readonly resource?: ResourceHandle | readonly ResourceHandle[];
  /** Alias of {@link TraitFields.resource}. */
  readonly resources?: ResourceHandle | readonly ResourceHandle[];
  /** Declared signals. @see {@link signal} */
  readonly signal?: SignalHandle | readonly SignalHandle[];
  /** Alias of {@link TraitFields.signal}. */
  readonly signals?: SignalHandle | readonly SignalHandle[];
  /** Declared sessions. @see {@link session} */
  readonly session?: SessionHandle | readonly SessionHandle[];
  /** Alias of {@link TraitFields.session}. */
  readonly sessions?: SessionHandle | readonly SessionHandle[];
  /** `[[scenario]]` declarations: product review behavior examples. */
  readonly scenario?: CanonicalScenario | readonly CanonicalScenario[];
  /** `[[eval]]` declarations: declared product evaluation checks. */
  readonly eval?: CanonicalEval | readonly CanonicalEval[];
  /**
   * `[sink.*]` declarations (task 0110): the host-owned closed sink set,
   * e.g. `sink: { sessionTitle: input.prompt\`Working on ${topic}\` }`. The
   * object-layer semantic authority — usable from a `variant({…})` shell
   * before/without the functional DSL. Mutually exclusive with an
   * `effect.session.title(...)` declaration made inside this trait's own
   * `procedure.from(...)` body; declaring both is a build error.
   * @see {@link effect}
   */
  readonly sink?: SinkFields;
  /**
   * The one sanctioned raw-JSON escape hatch: spread directly into the
   * canonical draft ahead of every other field, for canonical keys this
   * interface does not (yet) type. Prefer a typed field above whenever one
   * exists.
   */
  readonly unsafeJson?: JsonObject;
  /**
   * Declares this call as a native variant family instead of a single trait:
   * a (possibly nested) map of variant leaves, keyed by segment and joined
   * with `:` (e.g. `variants: { quick: variant.import("./quick.js").default(),
   * strict: variant.default({ thorough: variant({...}).default(), brief:
   * variant.import("./brief.js") }) }` — `quick` is the root map's default
   * child, and `thorough` is the default child of the nested `strict` map).
   * Every map level (including the root) must have exactly one default
   * child so a bare family or intermediate prefix resolves deterministically.
   * Mutually exclusive with authoring a single trait body directly — when
   * present, every other declaration field is read from each leaf's own
   * `variant(...)`/`variant.import(...)` definition instead of from this
   * call's fields. Never reaches canonical JSON.
   * @see {@link variant}
   */
  readonly variants?: TraitFamilyMap;
  /**
   * Not a canonical trait field. Status moved to the team-edited
   * `[package].status` (`draft | ready`) in the package's root `trait.toml`;
   * it is never authored on the canonical trait document itself.
   */
  readonly status?: never;
  /**
   * Not a canonical trait field. Trust is machine-local only, keyed by
   * canonical digest in `~/.config/ctx/trust.toml`; set it via
   * `ctx traits review --approve`/`--deny`, never on the canonical trait
   * document.
   */
  readonly trust?: never;
}

/**
 * Fields a `trait(id, { variants })` family call accepts: identity and
 * topology only. Every other declaration lives in each leaf's own
 * `variant(...)`/`variant.import(...)` definition — a family call is
 * mutually exclusive with authoring a trait body directly, so `trait()`
 * rejects any other `TraitFields` key at runtime.
 */
export interface FamilyFields {
  readonly id?: Slug;
  readonly version?: SemVer;
  readonly variants: TraitFamilyMap;
}

/** The only keys a `trait(id, { variants })` family call may supply. @see {@link FamilyFields} */
const FAMILY_OWNED_KEYS = new Set<string>(["id", "version", "variants"]);

/**
 * Assembles a complete trait draft from declarations and a procedure: the
 * top-level call every CDK trait source ends with, producing the
 * `TraitHandle` that `ctx traits build` normalizes to the canonical
 * `generated/index.toml` document.
 *
 * Authoring a trait document directly in TOML/JSON means every declared
 * agent/port/slot/prompt/sequence has to be hand-collected into the
 * right top-level array, in the right shape, with ids kept unique and
 * consistent across every place they're referenced — the exact bookkeeping
 * `trait(...)` automates by walking the fields it's given and collecting
 * every declaration reachable from them (ports referenced by a procedure,
 * schemas referenced by slots, etc.) rather than requiring an exhaustive
 * top-level list.
 *
 * @param fields Trait identity, declarations, and procedure.
 * @example
 * ```ts
 * const diff = port.input.text({ id: "diff" });
 * const focus = port.input.text({ id: "focus" });
 * const review = slot.text("review");
 * const output = port.output.of({ id: "review", schema: "schema:text", value: review });
 * const reviewStep = sequence.prompt("review", { text: input.prompt`Review ${diff} with a focus on ${focus}.`, output: review });
 * export default trait("code-review", {
 *   version: "0.1.0",
 *   summary: "Reviews a diff against a stated focus and returns a structured verdict.",
 *   procedure: procedure({
 *     description: "Review a diff for the stated focus.",
 *     sequence: reviewStep,
 *   }),
 * });
 * ```
 * @see {@link procedure}
 */
export function trait<const Id extends string>(id: ValidSlug<Id>, fields: Omit<FamilyFields, "id">): TraitFamilyHandle;
export function trait<const Fields extends FamilyFields>(
  fields: Fields & ValidTraitFields<Fields> & NoExtraFields<Fields, FamilyFields>,
): TraitFamilyHandle;
export function trait<const Id extends string>(id: ValidSlug<Id>, fields: Omit<TraitFields, "id">): TraitHandle;
export function trait<const Fields extends TraitFields>(
  fields: Fields & ValidTraitFields<Fields> & NoExtraFields<Fields, TraitFields>,
): TraitHandle;
export function trait(first: string | TraitFields, second?: Omit<TraitFields, "id">): TraitHandle | TraitFamilyHandle {
  const fields = canonicalTraitFields(
    (typeof first === "string" ? { ...second, id: first } : first) as TraitFields,
  ) as TraitFields;
  if (fields.variants !== undefined) {
    const rejectedKeys = Object.entries(fields)
      .filter(([key, value]) => value !== undefined && !FAMILY_OWNED_KEYS.has(key))
      .map(([key]) => key);
    if (rejectedKeys.length > 0) {
      throw new Error(
        `trait(): a variant family only accepts id/version/variants — remove ${rejectedKeys.join(", ")} ` +
          "(declare them in each leaf's own variant(...) definition instead)",
      );
    }
    return buildTraitFamily(fields);
  }
  const assembled = assembleSingleTraitDraft(fields);
  return withMeta(stableObject(assembled.draft), {
    kind: "trait",
    declaration: assembled.draft,
    declarations: assembled.merged,
    diagnostics: assembled.diagnostics,
    sourceMap: assembled.sourceMap,
  });
}

/**
 * The single-definition assembly shared by a plain `trait()` call and each
 * leaf of a `variants` family: walks a `TraitFields` value's declarations,
 * merges/dedupes them, and produces the canonical draft JSON plus its
 * diagnostics and source map. `trait()` wraps this once per call; family
 * assembly (`variant.ts`) invokes it once per resolved leaf, after
 * injecting the family's shared `id`/`version`/`schema-version`/`variant`.
 */
export function assembleSingleTraitDraft(fields: TraitFields): {
  readonly draft: CanonicalTraitDraft;
  readonly merged: ReturnType<typeof mergeDeclarationSets>;
  readonly diagnostics: readonly CdkDiagnostic[];
  readonly sourceMap: SourceMap;
} {
  const declarationSources = [
    fields.procedure,
    fields.sequence,
    fields.condition,
    fields.agent,
    fields.port,
    fields.slot,
    fields.setting,
    fields.prompt,
    fields.resource,
    fields.signal,
    fields.session,
  ];
  const collected = collectMany(declarationSources);
  const authored = explicitDeclarations(fields);
  resolveGeneratedBranchArmIds([fields.procedure, fields.sequence], collected, authored);
  const explicit = explicitDeclarations(fields);
  let merged = mergeDeclarationSets(collected, explicit);
  const inferred =
    fields.procedure === undefined
      ? undefined
      : inferProcedureContract(
          normalizeValue(fields.procedure),
          merged.sequence ?? [],
          explicit.port ?? [],
          merged.port ?? [],
          fields.procedure,
        );
  // Output ports are often declared beside their slot and need not be passed
  // through `trait({ port })`. Keep the selected declarations reachable in
  // this assembly so the emitted contract, schema closure, and source map all
  // describe one boundary. Do not modify the procedure handle: it can be
  // shared by independent traits with different boundary declarations.
  if (inferred?.ports.length) {
    merged = mergeDeclarationSets(merged, { port: inferred.ports });
  }
  const procedureValue = inferred?.procedure;
  const proceduralSessionTitleSink = readSessionTitleSink(fields.procedure);
  if (proceduralSessionTitleSink !== undefined && fields.sink?.sessionTitle !== undefined) {
    throw new Error(
      "[sink.session-title] declared twice: once via the object-layer `sink` field and once via `effect.session.title(...)` inside the procedure",
    );
  }
  const sessionTitleSinkInput = fields.sink?.sessionTitle ?? proceduralSessionTitleSink;
  const draft = compact({
    ...fields.unsafeJson,
    ...withoutKeys(fields as Record<string, unknown>, [
      "description",
      "behavior",
      "intent",
      "dependency",
      "procedure",
      "agent",
      "agents",
      "port",
      "ports",
      "slot",
      "slots",
      "prompt",
      "prompts",
      "sequence",
      "sequences",
      "condition",
      "conditions",
      "resource",
      "resources",
      "signal",
      "signals",
      "session",
      "sessions",
      "sink",
      "unsafeJson",
      "variants",
    ]),
    ...(sessionTitleSinkInput === undefined
      ? {}
      : { sink: { "session-title": sessionTitleSinkDraft(sessionTitleSinkInput) } }),
    "schema-version": fields["schema-version"] ?? "0.5",
    version: fields.version ?? "0.1.0",
    behavior: normalizeBehavior(fields.behavior),
    intent: normalizeIntent(fields.intent),
    description: descriptionFromTraitFields(fields),
    summary: summaryFromTraitFields(fields),
    ...(fields.dependency === undefined
      ? {}
      : { dependency: Array.isArray(fields.dependency) ? [...fields.dependency] : [fields.dependency] }),
    agent: mutableJsonArray(merged.agent),
    procedure: procedureValue,
    port: finalizedPortDeclarations(merged.port ?? []),
    slot: mutableJsonArray(merged.slot),
    setting: mutableJsonArray(merged.setting),
    prompt: promptMap(merged.prompt),
    sequence: sequenceLinearMap(merged.sequence),
    condition: conditionMap([fields.condition, merged.condition]),
    resource: mutableJsonArray(merged.resource),
    signal: mutableJsonArray(merged.signal),
    session: mutableJsonArray(merged.session),
    schema: mutableJsonArray(merged.schema),
    // P481 §4.5: the draft is assembled dynamically (spread + `withoutKeys`
    // over a runtime `TraitFields` record), so the producer side cannot be
    // statically checked without rewriting that assembly — one documented
    // cast at this return boundary; the type's payoff is at the consumer
    // boundary (`toDraftJson`'s `DraftOf<T>`).
  }) as CanonicalTraitDraft;
  return {
    draft,
    merged,
    diagnostics: collectDiagnostics(declarationSources),
    sourceMap: collectSourceMaps(declarationSources),
  };
}

/**
 * Derives a procedure boundary from the fully materialized procedure and the
 * declarations reachable within this trait assembly. Candidate declarations
 * are deliberately limited to this assembled leaf: process-global authoring
 * records can include sibling native-family leaves with overlapping ids.
 * They are selected only by an exact consumed port ref or produced bound-slot
 * ref, keeping unrelated declarations out of the leaf.
 */
function inferProcedureContract(
  procedure: JsonValue | undefined,
  sequences: readonly JsonObject[],
  authoredPorts: readonly JsonObject[],
  reachablePorts: readonly JsonObject[],
  procedureHandle: ProcedureHandle,
): { readonly procedure: JsonValue; readonly ports: readonly JsonObject[] } | undefined {
  if (procedure === undefined || typeof procedure !== "object" || Array.isArray(procedure)) {
    return procedure === undefined ? undefined : { procedure, ports: [] };
  }
  const consumed = new Set<string>();
  const produced = new Set<string>();
  const producedPorts = new Set<string>();
  const bySequence = new Map(
    sequences.flatMap((sequence) =>
      typeof sequence.id === "string" ? [[`sequence:${sequence.id}`, sequence] as const] : [],
    ),
  );
  const visitedSequences = new Set<string>();
  const visit = (value: JsonValue, key?: string): void => {
    if (typeof value === "string") {
      if (value.startsWith("port:")) consumed.add(value);
      if (key === "output" || key === "destination") {
        if (value.startsWith("slot:")) produced.add(value);
        if (value.startsWith("port:")) producedPorts.add(value);
      }
      if (value.startsWith("sequence:") && !visitedSequences.has(value)) {
        visitedSequences.add(value);
        const sequence = bySequence.get(value as `sequence:${string}`);
        if (sequence !== undefined) visit(sequence);
      }
      return;
    }
    if (Array.isArray(value)) return void value.forEach((item) => visit(item, key));
    if (value !== null && typeof value === "object") {
      Object.entries(value).forEach(([childKey, child]) => {
        if (child !== undefined) visit(child, childKey);
      });
    }
  };
  visit(procedure);
  const procedurePorts = metaOf(procedureHandle)?.declarations?.port ?? [];
  const ports =
    mergeDeclarationSets({ port: authoredPorts }, { port: reachablePorts }, { port: procedurePorts }).port ?? [];
  const input: string[] = [];
  const output: string[] = [];
  for (const port of ports) {
    const id = typeof port.id === "string" ? port.id : undefined;
    if (id === undefined) continue;
    const ref = `port:${id}`;
    if (port.direction === "input" && consumed.has(ref)) input.push(ref);
    if (
      port.direction === "output" &&
      (producedPorts.has(ref) || (typeof port.value === "string" && produced.has(port.value)))
    ) {
      output.push(ref);
    }
  }
  const selected = ports.filter((port) => {
    const id = typeof port.id === "string" ? `port:${port.id}` : undefined;
    return id !== undefined && (input.includes(id) || output.includes(id));
  });
  return {
    procedure: compact({
      ...procedure,
      input: input.length === 0 ? undefined : input,
      output: output.length === 0 ? undefined : output,
    }),
    ports: selected,
  };
}

export type CustomSlug = string & { readonly __customSlugBrand: never };
/** A lowercase trait identifier. Literal ids are checked for slug syntax by `trait`. */
export type Slug = Lowercase<string>;
/**
 * The canonical trait document schema version. `"0.5"` is reserved for
 * native-variant leaves (`variant`/`variant.import` under `trait(id, {
 * variants })`) and is injected automatically — never authored directly.
 */
export type SchemaVersion = "0.2" | "0.3" | "0.4" | "0.5";
/** A three-component semantic version for trait releases. */
export type SemVer = `${number}.${number}.${number}`;
export type Tone = GuidanceInput<ToneBuiltIn>;
export type Method = GuidanceInput<MethodBuiltIn>;
export type Verbosity = GuidanceInput<VerbosityBuiltIn>;

type SlugCharacter =
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
  | "0"
  | "1"
  | "2"
  | "3"
  | "4"
  | "5"
  | "6"
  | "7"
  | "8"
  | "9";
type ValidSlugTail<Value extends string> = Value extends ""
  ? true
  : Value extends `${SlugCharacter}${infer Rest}`
    ? ValidSlugTail<Rest>
    : Value extends `-${infer Rest}`
      ? Rest extends "" | `-${string}`
        ? false
        : ValidSlugTail<Rest>
      : false;
type ValidSlug<Value extends string> = string extends Value
  ? Value
  : Value extends `${SlugCharacter}${infer Rest}`
    ? ValidSlugTail<Rest> extends true
      ? Value
      : never
    : never;
type ValidTraitFields<Fields extends { readonly id?: string }> = Fields["id"] extends string
  ? ValidSlug<Fields["id"]> extends never
    ? never
    : unknown
  : unknown;
/** Generic object-form overloads need this explicit excess-key guard. */
type NoExtraFields<Fields, Shape> = Record<Exclude<keyof Fields, keyof Shape>, never>;

type CallableNamespace<Values extends Record<string, string>> = ((value: string) => CustomSlug) &
  PascalCaseLeaves<Values>;

/**
 * Builds a lowercase callable namespace for a flat generated vocabulary map:
 * calling it validates a custom slug, and its PascalCase properties resolve
 * to generated built-in values.
 */
function callableNamespace<Values extends Record<string, string>>(values: Values): CallableNamespace<Values> {
  return Object.assign((value: string): CustomSlug => customSlug(value), pascalCaseLeaves(values));
}

type PascalCase<Key extends string> = Key extends `${infer Head}${infer Rest}` ? `${Uppercase<Head>}${Rest}` : Key;
type PascalCaseLeaves<Values extends Record<string, string>> = {
  [Key in keyof Values as PascalCase<Key & string>]: Values[Key];
};

/**
 * The behavior vocabulary, grouped by the axis each value belongs to:
 * `behavior.tone.Direct`, `behavior.method.EvidenceFirst`,
 * `behavior.verbosity.Brief`, and so on for every axis.
 *
 * Each axis is also callable, to build a validated custom slug alongside the
 * generated built-ins. An axis accepts any lowercase slug, not just the
 * catalog's — calling it validates the slug shape at build time instead of
 * letting an arbitrary string through to fail synth with a vaguer error.
 *
 * @example `useBehavior({ tone: [behavior.tone.Direct], method: behavior.method.EvidenceFirst })`
 * @example `useBehavior({ tone: [behavior.tone("blunt")] })`
 */
export const behavior = {
  tone: callableNamespace(GeneratedTone),
  method: callableNamespace(GeneratedMethod),
  verbosity: callableNamespace(GeneratedVerbosity),
  directness: callableNamespace(GeneratedDirectness),
  scopeControl: callableNamespace(GeneratedScopeControl),
  initiative: callableNamespace(GeneratedInitiative),
  uncertainty: callableNamespace(GeneratedUncertainty),
  format: callableNamespace(GeneratedFormat),
} as const;

/**
 * The intent vocabulary, flat: `intent.Leanness`, `intent.ScopeCreep`.
 *
 * The catalog says what an item IS. Whether it is a require, a focus, an
 * avoid, or a block is the trait's decision at its own call site, so the same
 * item reads correctly in any facet — `avoid: [intent.ScopeCreep]` and
 * `focus: [intent.ScopeCreep]` both mean something, and mean different things.
 *
 * Callable to build a validated custom slug, same as the behavior axes.
 *
 * @example `useIntent({ require: [intent.Leanness], avoid: [intent.ScopeCreep] })`
 * @example `useIntent({ require: [intent("cite-evidence")] })`
 */
export const intent = callableNamespace(GeneratedIntent);



function pascalKey(key: string): string {
  return key.charAt(0).toUpperCase() + key.slice(1);
}
/** Adds PascalCase leaves to a lower-camel-case generated vocabulary map. */
function pascalCaseLeaves<Values extends Record<string, string>>(values: Values): PascalCaseLeaves<Values> {
  const aliases: Record<string, string> = {};
  for (const [key, value] of Object.entries(values)) {
    aliases[pascalKey(key)] = value;
  }
  return aliases as PascalCaseLeaves<Values>;
}

function customSlug<T extends string>(value: T): T & CustomSlug {
  if (!isSlug(value)) {
    throw new Error(`custom slug: expected lowercase slug, got ${JSON.stringify(value)}`);
  }
  return value as T & CustomSlug;
}
