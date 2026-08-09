import { recordTraitMint } from "./functional/context.js";
import type {
  CanonicalChecklistItem,
  CanonicalDependency,
  CanonicalRule,
  JsonObject,
  JsonValue,
  ResourceRender,
  ResourceRoot,
  ResourceTrigger,
} from "./generated.js";
import type { ProcedureHandle, ResourceHandle, SchemaHandle, SequenceHandle, SignalHandle } from "./handles.js";
import { withDeclaration, withMeta } from "./meta.js";
import {
  collectDiagnostics,
  collectMany,
  compact,
  compactAs,
  headingBlock,
  sourceMapForSequenceItems,
  validateSlug,
} from "./normalize.js";
import { schema } from "./schema.js";
import { materializeSequenceItem, normalizeMaterializedSequenceItem, validateNoDuplicateTitles } from "./sequence.js";

/** Scalar-or-array predicate list, matching the canonical `PredicateList` authoring shape. */
type PredicateFields = string | readonly string[];

/**
 * Typed fields for a `[[activation.rule]]` declaration. Rules compose as OR;
 * inside one rule, the non-empty positive predicate fields compose as AND.
 * `excludeFileGlob`/`excludeKeyword` block the rule before positive scoring.
 * @see {@link rule}
 */
/** Preferred initial load level: `"discovery"`, `"summary"`, or `"full"`. */
export type RuleLoadLevel = "discovery" | "summary" | "full";

export interface RuleFields {
  /** Stable rule id, unique within `[activation]`. */
  readonly id: string;
  /** Human-readable reason shown by explanation surfaces. */
  readonly reason: string;
  readonly mode?: PredicateFields;
  readonly language?: PredicateFields;
  readonly fileGlob?: PredicateFields;
  readonly taskKeyword?: PredicateFields;
  readonly signal?: PredicateFields;
  readonly explicitPhrase?: PredicateFields;
  readonly excludeFileGlob?: PredicateFields;
  readonly excludeKeyword?: PredicateFields;
  readonly weight?: number;
  readonly loadLevel?: RuleLoadLevel;
}

/** Fields for a `[[signal]]` declaration. `description` is required by the canonical schema. */
export interface SignalFields {
  readonly id: string;
  readonly description: string;
}

/** A dependency resolved from a local path relative to the depending package. */
export interface DependencyLocalSource {
  readonly path: string;
  readonly git?: never;
  readonly ref?: never;
  readonly registry?: never;
  readonly package?: never;
}
/** A dependency resolved directly from a Git repository. */
export interface DependencyGitSource {
  readonly git: string;
  readonly ref?: string;
  readonly path?: string;
  readonly registry?: never;
  readonly package?: never;
}
/** A dependency resolved from the npm registry; `registry` defaults to `"npm"` when omitted. */
export interface DependencyNpmSource {
  readonly package: string;
  readonly registry?: "npm";
  readonly path?: string;
  readonly git?: never;
  readonly ref?: never;
}
/** Optional `dependency.source` metadata: local path, direct Git, or npm package — never more than one at once. */
export type DependencySource = DependencyLocalSource | DependencyGitSource | DependencyNpmSource;

/** Typed fields for a `[[dependency]]` declaration. @see {@link dependency} */
export interface DependencyFields {
  /** Typed reference namespace for this dependency (e.g. `"aws"`). Must be unique within the manifest. */
  readonly alias: string;
  /** The trait ID being depended on (e.g. `"ctx/aws-log-primitives"`). */
  readonly id: string;
  /** The trait version (e.g. `"0.1.0"`). */
  readonly version: string;
  /** Optional source metadata; when omitted, resolved from the package-local `trait.lock`. */
  readonly source?: DependencySource;
}

export interface ProcedureFields {
  readonly description: string;
  readonly worktreeRequired?: boolean;
  readonly sequenceOrder?: readonly string[];
  readonly sequence?: SequenceHandle | readonly SequenceHandle[] | readonly JsonObject[];
}

/**
 * Declares a trait dependency: another trait this trait's refs
 * (`ref.slot({ id, dependency })`, `refText`-qualified handles, etc.) may
 * reach into.
 *
 * @param fields Dependency declaration fields.
 * @example `dependency({ alias: "shared", id: "ctx/shared", version: "0.1.0", source: { path: "../shared" } })`
 * @see {@link trait}
 */
export function dependency(fields: DependencyFields): CanonicalDependency {
  return compactAs<CanonicalDependency>({ ...fields, source: fields.source as JsonValue | undefined });
}
/**
 * Assembles a procedure from sequence steps. Its contract is inferred from
 * consumed input ports and output ports bound to produced slots.
 *
 * Passing the slot/sequence handles built elsewhere in the trait is how a
 * step's `input`/`output` refs stay
 * traceable to a real declaration instead of an untyped string. But
 * `sequence` also accepts raw `JsonObject[]`, so `procedure(...)` itself does not enforce
 * this at `tsc` — a mismatched or misspelled ref passed here only surfaces
 * at synth or run time. The `tsc`-checked guarantee lives one level down,
 * at the individual `sequence.*` step builders, whose `input`/`output`
 * fields are typed against the handles in scope.
 *
 * @param fields Procedure declaration fields.
 * @example
 * ```ts
 * procedure({
 *   description: "Review a diff for the stated focus and return one structured verdict with findings.",
 *   sequence: sequence.prompt("review", { agent: reviewer, text: input.prompt`Review ${diff}.`, output: review }),
 * });
 * ```
 * @see {@link sequence}
 */
export function procedure(fields: ProcedureFields): ProcedureHandle {
  const sequenceValues = arrayOf<SequenceHandle | JsonObject>(fields.sequence).flatMap((item) =>
    materializeSequenceItem(item as SequenceHandle, { kind: "procedure" })
  );
  validateNoDuplicateTitles(sequenceValues, "procedure");
  const items = sequenceValues.map(normalizeMaterializedSequenceItem);
  const sourceMap = sourceMapForSequenceItems("procedure", sequenceValues, items);
  return withMeta(
    compact({
      description: fields.description,
      "worktree-required": fields.worktreeRequired,
      "sequence-order": fields.sequenceOrder === undefined ? undefined : Array.from(fields.sequenceOrder),
      sequence: items,
    }),
    {
      kind: "procedure",
      declarations: collectMany([sequenceValues]),
      diagnostics: collectDiagnostics([sequenceValues]),
      sourceMap,
    },
  );
}
/**
 * Mirrors `CanonicalResource`'s key set exactly (`generated.ts:1180-1244`)
 * — the escape hatch this interface types spans precisely the canonical
 * resource surface, no more. A misspelled or invented key (previously
 * absorbed by an `extends JsonObject` index signature) now fails to
 * typecheck instead of silently reaching the canonical document.
 */
export interface ResourceFields {
  readonly id: string;
  readonly path?: string;
  /** Root `path` resolves against; only meaningful alongside `path`. Defaults to `package`. */
  readonly root?: ResourceRoot;
  readonly content?: string;
  /** Presentation mode: `reference` (open a path) or `inline` (embed the body). Defaults by source: `reference` for `path`, `inline` for `content`. */
  readonly render?: ResourceRender;
  /**
   * Canonical pin over the file's expected bytes, `sha256:<hex>`. Presence
   * marks the resource protected: Rust verifies actual bytes against this
   * pin immediately before the resource reaches a model or a command spawns,
   * and `ctx traits check` reports drift between this pin and the file on
   * disk. Only meaningful alongside `path` — a resource has no filesystem
   * bytes to pin without one.
   */
  readonly digest?: string;
  /** Advisory hint about the resource contents or usage. */
  readonly hint?: string;
  /** Optional resource kind. `template` resources may declare typed inputs. */
  readonly variant?: string;
  /** When to load the resource at runtime. */
  readonly trigger?: ResourceTrigger;
  /** Typed inputs required by a resource template. */
  readonly input?: string | readonly string[];
  /**
   * Declared checklist items. Valid only alongside `variant: "checklist"`,
   * where they replace `path`/`content` as the resource body.
   */
  readonly item?: readonly CanonicalChecklistItem[];
}
/**
 * Resource builder call signatures: declares content (a file on disk, or
 * inline text) that prompts and procedures can reference by a stable id —
 * the "guide" or "rubric" doctrine text a prompt interpolates instead of
 * embedding directly.
 *
 * A resource declared once and referenced by the returned handle means
 * editing its content in one place updates every prompt that reads it, and
 * a stale reference to a renamed handle fails at `tsc`. Referencing a
 * resource by its bare string id instead is not checked by `tsc` at all — a
 * typo there only surfaces when the Rust synth/runtime looks up the id and
 * finds nothing declared. `resource.file`
 * binds a path on disk (resolved relative to `root`, default `package`);
 * `resource.inline` embeds content directly in the trait source, which is
 * what `checklist`/`rubric`/`steps`/`table`/etc. build on top of.
 *
 * @example `resource.inline("guide", "Prefer minimal diffs.")`
 * @see {@link prompt}
 */
export interface ResourceFunction {
  /** Raw resource declaration: the escape hatch when neither `file` nor `inline` fits. @example `resource({ id: "guide", content: "Prefer minimal diffs." })` */
  (fields: ResourceFields): ResourceHandle;
  /**
   * Binds a file on disk, resolved relative to `root` (default `package`).
   * Pass `digest` to pin the file's expected bytes — required before the
   * resource can be referenced as command argv (see {@link sequence}).
   * @example `resource.file("style-guide", { path: "docs/style.md" })`
   * @example `resource.file("evaluator", { path: "scripts/eval.py", digest: "sha256:..." })`
   */
  file(id: string, fields: Omit<ResourceFields, "id" | "content"> & { readonly path: string; }): ResourceHandle;
  /** Embeds content directly in the trait source, no file on disk. @example `resource.inline("guide", "Prefer minimal diffs.")` */
  inline(
    id: string,
    content: string,
    fields?: Omit<ResourceFields, "id" | "path" | "root" | "content">,
  ): ResourceHandle;
}
export interface ChecklistFields {
  readonly id: string;
  readonly title?: string;
  readonly items: readonly string[];
}
export interface RubricFields {
  readonly id: string;
  readonly title?: string;
  readonly items: readonly string[];
}

/** One criterion in a typed checklist: the unit that receives one verdict. */
export interface ChecklistItemFields {
  /** Stable identity, kept across rewordings of `text`. */
  readonly id: string;
  /** The criterion as the reviewer reads it. */
  readonly text: string;
  /** Elaboration shown with the item. Never separately verdict-bearing. */
  readonly detail?: string;
  /** Require this item's verdict to cite evidence. */
  readonly requiresEvidence?: boolean;
}

export interface TypedChecklistFields<T extends readonly ChecklistItemFields[]> {
  readonly id: string;
  readonly items: T;
  readonly hint?: string;
}

/**
 * A typed checklist resource plus the verdict schema that answers it.
 *
 * `Ids` is the union of declared item ids, so a reference to an item that does
 * not exist fails at `tsc` — before `build`, before the Rust validator, and
 * long before a model is asked to answer a criterion nobody wrote.
 */
export interface ChecklistHandle<Ids extends string = string> extends ResourceHandle {
  /** The verdict schema for this checklist: one verdict for one item. */
  readonly verdict: SchemaHandle;
  /** Declared item ids, in declaration order. */
  readonly itemIds: readonly Ids[];
}

/** The statuses a checklist verdict may carry. */
export const CHECKLIST_STATUSES = ["pass", "fail", "waived"] as const;

/**
 * Declares a checklist.
 *
 * Passing plain strings renders a markdown block — convenient prose, and
 * nothing more: the items reach the model as bullets and no part of the
 * runtime can name them. Passing item objects declares them as typed data, and
 * the returned handle carries a verdict schema whose `item` field is the closed
 * set of declared ids, which is what lets the runtime check that every item was
 * actually answered.
 *
 * @example `checklist({ id: "rubric", items: [{ id: "no-unwrap", text: "No .unwrap() in lib code" }] })`
 */
export function checklist(fields: ChecklistFields): ResourceHandle;
export function checklist<const T extends readonly ChecklistItemFields[]>(
  fields: TypedChecklistFields<T>,
): ChecklistHandle<T[number]["id"]>;
export function checklist(
  fields: ChecklistFields | TypedChecklistFields<readonly ChecklistItemFields[]>,
): ResourceHandle {
  const [first] = fields.items;
  if (first === undefined || typeof first === "string") {
    const prose = fields as ChecklistFields;
    return resource.inline(prose.id, formatBulletList(prose.title ?? "Checklist", prose.items));
  }
  return typedChecklist(fields as TypedChecklistFields<readonly ChecklistItemFields[]>);
}

/**
 * Creates an inline-content Markdown rubric resource — prose bullets under a
 * heading, for evaluation criteria that don't need per-item typed verdicts.
 * Reach for `checklist`'s typed form instead when the runtime needs to check
 * that every item was actually answered.
 * @example `rubric({ id: "style-rubric", items: ["Consistent naming", "No dead code"] })`
 * @see {@link checklist}
 */
export function rubric(fields: RubricFields): ResourceHandle {
  return resource.inline(fields.id, formatBulletList(fields.title ?? "Rubric", fields.items));
}

function typedChecklist(fields: TypedChecklistFields<readonly ChecklistItemFields[]>): ChecklistHandle {
  validateSlug(fields.id, "checklist.id");
  if (fields.items.length === 0) {
    throw new Error(`checklist ${JSON.stringify(fields.id)}: must declare at least one item`);
  }

  const seen = new Set<string>();
  const items = fields.items.map((item, index) => {
    const itemPath = `checklist.${fields.id}.item[${index}].id`;
    validateSlug(item.id, itemPath);
    if (seen.has(item.id)) {
      throw new Error(`${itemPath}: duplicate checklist item id ${JSON.stringify(item.id)}`);
    }
    seen.add(item.id);
    if (item.text.trim() === "") {
      throw new Error(`checklist.${fields.id}.item[${index}].text: must not be empty`);
    }
    return compact({
      id: item.id,
      text: item.text,
      detail: item.detail,
      "requires-evidence": item.requiresEvidence,
    });
  });

  const itemIds = fields.items.map((item) => item.id);
  // Evidence binds the whole verdict shape as soon as one item asks for it: a
  // single object schema cannot make a field conditionally required, so the
  // strictest item sets the contract. Core checks this same rule.
  const requiresEvidence = fields.items.some((item) => item.requiresEvidence === true);

  const verdict = schema.object(`${fields.id}-verdict`, {
    item: schema.field(schema.enum(itemIds), {
      description: "Which checklist item this verdict answers.",
    }),
    status: schema.field(schema.enum(CHECKLIST_STATUSES), {
      description: "The verdict for this item.",
    }),
    evidence: schema.field(schema.text(), {
      required: requiresEvidence,
      description: "Concrete evidence supporting the status.",
    }),
  }, { description: `One verdict for one item of checklist resource:${fields.id}.` });

  const declaration = compact({
    id: fields.id,
    variant: "checklist",
    hint: fields.hint,
    item: items,
  });

  // `verdict`/`itemIds` are attached below via `Object.defineProperty`, after
  // this call returns — no expression at this point structurally has them,
  // so the mint of the wider `ChecklistHandle` shape is unavoidable here.
  const handle = withDeclaration("resource", `resource:${fields.id}`, declaration, {}, {
    declarations: collectMany([verdict]),
  }) as ChecklistHandle;
  recordTraitMint("resource", fields.id, `resource:${fields.id}`, declaration);

  // Authoring affordances, not manifest fields: the declaration object is
  // emitted verbatim, so anything enumerable on it becomes an unknown field.
  Object.defineProperty(handle, "verdict", { value: verdict, enumerable: false });
  Object.defineProperty(handle, "itemIds", { value: itemIds, enumerable: false });
  return handle;
}
export const resource: ResourceFunction = Object.assign(
  (fields: ResourceFields): ResourceHandle => resourceOf(fields),
  {
    file: (id: string, fields: Omit<ResourceFields, "id" | "content"> & { readonly path: string; }): ResourceHandle =>
      resourceOf({ ...fields, id }),
    inline: (
      id: string,
      content: string,
      fields: Omit<ResourceFields, "id" | "path" | "root" | "content"> = {},
    ): ResourceHandle => resourceOf({ ...fields, id, content }),
  },
);

function resourceOf(fields: ResourceFields): ResourceHandle {
  const declaration = compact({ ...fields });
  const handle = withDeclaration("resource", `resource:${String(fields.id)}`, declaration, {});
  recordTraitMint("resource", String(fields.id), `resource:${String(fields.id)}`, declaration);
  return handle;
}

function formatBulletList(title: string, items: readonly string[]): string {
  return headingBlock(title, items.map((item) => `- ${item}`).join("\n"));
}
/** Normalizes a scalar-or-array predicate field into the canonical always-array form, preserving `undefined`. */
function predicateList(value: PredicateFields | undefined): readonly string[] | undefined {
  return value === undefined ? undefined : typeof value === "string" ? [value] : value;
}

/**
 * Declares an `[[activation.rule]]` entry: a candidate-matching predicate
 * for deterministic activation scoring.
 * @param fields Rule declaration fields.
 * @example `rule({ id: "safe", fileGlob: "**\/*.rs" })`
 * @see {@link trait}
 */
export function rule(fields: RuleFields): CanonicalRule {
  return compactAs<CanonicalRule>({
    id: fields.id,
    reason: fields.reason,
    mode: predicateList(fields.mode),
    language: predicateList(fields.language),
    "file-glob": predicateList(fields.fileGlob),
    "task-keyword": predicateList(fields.taskKeyword),
    signal: predicateList(fields.signal),
    "explicit-phrase": predicateList(fields.explicitPhrase),
    "exclude-file-glob": predicateList(fields.excludeFileGlob),
    "exclude-keyword": predicateList(fields.excludeKeyword),
    weight: fields.weight,
    "load-level": fields.loadLevel,
  });
}
/**
 * Declares a signal emitted by sequence steps: a named event a step's
 * `onComplete` field raises, that guards elsewhere (`condition.signal`) or
 * `onFailure` routes can react to.
 *
 * Without a declared signal, a step's `onComplete`/`onFailure` targets are bare
 * strings with no check that anything actually emits (or listens for) that
 * name. `signal(...)` returns a typed handle usable on both sides.
 *
 * @param fields Signal declaration fields. `description` is required by the
 * canonical schema (states when the signal fires), unlike most other
 * declaration builders where it's optional.
 * @example `signal({ id: "approved", description: "The reviewer approved the change." })`
 * @see {@link sequence}
 */
export function signal(fields: SignalFields): SignalHandle {
  const declaration = compact({ ...fields });
  const handle = withDeclaration("signal", `signal:${fields.id}`, declaration, {});
  recordTraitMint("signal", fields.id, `signal:${fields.id}`, declaration);
  return handle;
}

/**
 * `T` deliberately not narrowed to `SequenceHandle`: `ProcedureFields.sequence`'s declared type
 * (`SequenceHandle | readonly SequenceHandle[] | readonly JsonObject[]`) also accepts a raw
 * canonical-JSON step, which every call site here still treats as a `SequenceHandle` by
 * construction (JSON step objects round-trip through `metaOf` the same way handles do) — a real
 * generic signature would have to model that overlap, not just rename `unknown`.
 */
function arrayOf<T>(value: T | readonly T[] | undefined): readonly T[] {
  if (value === undefined) return [];
  return Array.isArray(value) ? value as readonly T[] : [value as T];
}
