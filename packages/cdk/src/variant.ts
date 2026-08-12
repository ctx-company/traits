import type {
  FlattenedVariant,
  TraitFamilyMap,
  VariantHandle,
  VariantHandleBase,
  VariantImportHandle,
  VariantLeafHandle,
  VariantTopologyNode,
} from "./authoring-types.js";
import { VARIANT_HANDLE } from "./authoring-types.js";
import type { CanonicalTraitDraft } from "./generated.js";
import type { TraitFamilyHandle } from "./handles.js";
import type { CdkDiagnostic, JsonObject, SourceMap } from "./meta.js";
import { captureSourceAnchor, metaOf, withMeta } from "./meta.js";
import type { SourceAnchor } from "./meta.js";
import { canonicalTraitFields, isSlug, sourceMapFor, stableObject } from "./normalize.js";
import { assembleSingleTraitDraft } from "./trait.js";
import type { SemVer, Slug, TraitFields } from "./trait.js";

export type {
  FlattenedVariant,
  TraitFamilyMap,
  VariantHandle,
  VariantImportHandle,
  VariantLeafHandle,
  VariantTopologyNode,
} from "./authoring-types.js";

/**
 * Fields for one complete variant definition: everything `trait()` accepts
 * except `id`/`version`/`schema-version` — those are injected from the
 * containing family (shared `id`/`version`, and the next canonical
 * schema version) when the variant is resolved for build.
 */
export type VariantFields = Omit<TraitFields, "id" | "version" | "schema-version" | "variants">;

interface VariantRecord {
  readonly kind: "definition" | "import";
  readonly fields?: VariantFields;
  readonly modulePath?: string;
  readonly sourceAnchor: SourceAnchor | undefined;
}

const variantRecords = new Map<string, VariantRecord>();
const defaultMarks = new WeakSet<object>();
let variantSeq = 0;

/**
 * Marks a value as the default child of its containing `variants` map level.
 * A variant handle exposes the same marking as chainable `.default()`; a
 * nested map is a plain object literal with no method of its own, so mark it
 * by wrapping it: `strict: variant.default({ thorough: ..., brief: ... })`.
 * Both forms record the same mark, read by `flattenVariantMap`.
 */
function markDefault<T extends object>(value: T): T {
  defaultMarks.add(value);
  return value;
}

function registerVariant<H extends VariantHandleBase>(record: VariantRecord): H {
  const id = `variant-${++variantSeq}`;
  variantRecords.set(id, record);
  const handle = {
    [VARIANT_HANDLE]: id,
    default(): H {
      return markDefault(handle);
    },
    // The single audited cast: `H` is the caller's opaque handle shape
    // (`VariantHandle`/`VariantImportHandle`/`VariantLeafHandle`), which
    // this factory can't structurally satisfy beyond the brand + `default()`
    // every variant handle shares (P485 Phase 2).
  } as unknown as H; // audited-unknown-cast: opaque caller-supplied handle shape, see comment above
  return handle;
}

function isVariantHandle(value: unknown): value is VariantLeafHandle {
  return (
    typeof value === "object" &&
    value !== null &&
    VARIANT_HANDLE in (value as object) &&
    variantRecords.has((value as VariantHandleBase)[VARIANT_HANDLE])
  );
}

function variantRecordOf(handle: VariantLeafHandle): VariantRecord {
  const record = variantRecords.get(handle[VARIANT_HANDLE]);
  if (record === undefined) {
    throw new Error("variant: handle not created via variant() or variant.import()");
  }
  return record;
}

/**
 * Declares one complete variant definition — the same shape `trait()`
 * accepts, minus `id`/`version`/`schema-version`. Place it as a value in a
 * `variants` map (nested maps join keys with `:`); call `.default()` to mark
 * it the default variant at its containing map level.
 * @example
 * ```ts
 * export default trait("review", {
 *   variants: {
 *     quick: variant({ summary: "Fast pass.", procedure: quickProcedure }).default(),
 *     strict: variant.import("./strict.js"),
 *   },
 * });
 * ```
 * @see {@link trait}
 */
export function variant(fields: VariantFields): VariantHandle {
  return registerVariant<VariantHandle>({ kind: "definition", fields, sourceAnchor: captureSourceAnchor() });
}

/**
 * Resolves an authored variant module relative to the calling trait
 * source's own location (not `@ctx-traits/cdk`'s install location) — its
 * default export must itself be a `variant({...})` or `variant.import(...)`
 * handle. Resolution happens lazily, when the family is flattened for
 * build, not at authoring time.
 */
variant.import = function importVariant(path: string): VariantImportHandle {
  const sourceAnchor = captureSourceAnchor();
  if (sourceAnchor === undefined) {
    throw new Error("variant.import: could not capture the calling source location");
  }
  return registerVariant<VariantImportHandle>({ kind: "import", modulePath: path, sourceAnchor });
};

/** Marks a nested `variants` map as the default child of its containing level. @see {@link markDefault} */
variant.default = markDefault;

interface FlattenResult {
  readonly variants: readonly FlattenedVariant[];
  readonly topology: VariantTopologyNode;
}

/**
 * Walks a `variants` map in stable (declaration) order, joining nested keys
 * with `:`, validating exactly one default child at every level (including
 * this one) and that every segment is a safe, validated slug.
 */
function flattenVariantMap(map: TraitFamilyMap, segments: readonly string[]): FlattenResult {
  const keys = Object.keys(map);
  if (keys.length === 0) {
    throw new Error(`variants${describePath(segments)}: map has no children`);
  }
  const variants: FlattenedVariant[] = [];
  const children: Record<string, VariantTopologyNode> = {};
  let defaultKey: string | undefined;
  for (const key of keys) {
    if (!isSlug(key)) {
      throw new Error(`variants${describePath(segments)}: invalid variant segment ${JSON.stringify(key)}`);
    }
    const childSegments = [...segments, key];
    const node = map[key] as VariantLeafHandle | TraitFamilyMap;
    if (isVariantHandle(node)) {
      variants.push({ path: childSegments.join(":"), segments: childSegments, handle: node });
      children[key] = { kind: "variant", path: childSegments.join(":") };
      if (defaultMarks.has(node)) {
        if (defaultKey !== undefined) {
          throw new Error(`variants${describePath(segments)}: multiple default children (${defaultKey}, ${key})`);
        }
        defaultKey = key;
      }
    } else {
      const nested = flattenVariantMap(node, childSegments);
      variants.push(...nested.variants);
      children[key] = nested.topology;
      if (defaultMarks.has(node)) {
        if (defaultKey !== undefined) {
          throw new Error(`variants${describePath(segments)}: multiple default children (${defaultKey}, ${key})`);
        }
        defaultKey = key;
      }
    }
  }
  if (defaultKey === undefined) {
    throw new Error(`variants${describePath(segments)}: missing a default child (call \`.default()\` on exactly one)`);
  }
  return { variants, topology: { kind: "map", default: defaultKey, children } };
}

function describePath(segments: readonly string[]): string {
  return segments.length === 0 ? "" : ` (${segments.join(":")})`;
}

/**
 * Builds a `TraitFamilyHandle` from a `trait(id, { variants })` call: flattens
 * and validates the map, but does not resolve `variant.import(...)` modules
 * yet — that's `resolveTraitFamily`, called at build time once the family's
 * variants are actually needed.
 */
export function buildTraitFamily(fields: TraitFields): TraitFamilyHandle {
  const id = fields.id;
  if (id === undefined) {
    throw new Error("trait(): a variant family requires an id");
  }
  const version = fields.version ?? "0.1.0";
  const { variants, topology } = flattenVariantMap(fields.variants as TraitFamilyMap, []);
  // No `kind` here: sdk-generate validates `Meta.kind`'s literal union against
  // a fixed Rust-side allow-list, and a family handle never needs kind-based
  // dispatch — `metaOf(handle)?.family` alone identifies it.
  return withMeta<JsonObject, string, unknown, "trait-family">(stableObject({}) as JsonObject, {
    family: { id, version, topology, variants },
  });
}

/** One resolved variant's complete draft, ready to synthesize. */
export interface ResolvedFamilyVariant {
  readonly path: string;
  readonly draft: CanonicalTraitDraft;
  readonly sourceMap: SourceMap;
  readonly diagnostics: readonly CdkDiagnostic[];
}

/** The tagged family envelope a build tool writes: one draft/map per resolved variant, plus the manifest topology. */
export interface FamilyEnvelope {
  readonly family: true;
  readonly id: string;
  readonly version: string;
  readonly topology: VariantTopologyNode;
  readonly variants: readonly ResolvedFamilyVariant[];
}

/** Whether a value is the opaque handle returned by `trait(id, { variants })`. */
export function isTraitFamilyHandle(value: unknown): value is TraitFamilyHandle {
  return metaOf(value)?.family !== undefined;
}

/** The next canonical schema version, used for every native-variant regardless of the family's authored `schema-version`. */
export const NATIVE_VARIANT_SCHEMA_VERSION = "0.4" as const;

/**
 * Resolves every `variant.import(...)` variant (relative to the module that
 * called it) and assembles each variant's complete draft, injecting the
 * shared family `id`/`version`, the fixed native-variant schema version, and
 * the variant's own colon-joined `variant` path. Returns the tagged envelope
 * a build tool writes as one canonical/map pair per variant.
 */
export async function resolveTraitFamily(handle: TraitFamilyHandle): Promise<FamilyEnvelope> {
  const family = metaOf(handle)?.family;
  if (family === undefined) {
    throw new Error("resolveTraitFamily: handle was not created by trait(id, { variants })");
  }
  const meta = family;
  const variants: ResolvedFamilyVariant[] = [];
  for (const variant of meta.variants) {
    const resolved = await resolveVariantFields(variant.handle);
    if (resolved.sourceAnchor === undefined) {
      throw new Error(`variant(): could not capture the authoring source location for variant ${variant.path}`);
    }
    const resolvedFields = canonicalTraitFields(resolved.fields as TraitFields);
    const assembled = assembleSingleTraitDraft({
      ...resolvedFields,
      id: meta.id as Slug,
      version: meta.version as SemVer,
      "schema-version": NATIVE_VARIANT_SCHEMA_VERSION,
    });
    const draft = stableObject({ ...assembled.draft, variant: variant.path });
    // Route through the same finalizer `toDraftJsonWithSourceMap` uses for an
    // ordinary trait, so a family variant gets the top-level `trait:<id>`
    // anchor too — passing `source` explicitly (the variant's own authored
    // location) rather than letting `withMeta` auto-capture the build tool's
    // frame.
    const variantHandle = withMeta(stableObject({}) as JsonObject, {
      kind: "trait",
      declaration: assembled.draft,
      declarations: assembled.merged,
      diagnostics: assembled.diagnostics,
      sourceMap: assembled.sourceMap,
      source: resolved.sourceAnchor,
    });
    variants.push({
      path: variant.path,
      draft,
      sourceMap: sourceMapFor(variantHandle, draft),
      diagnostics: assembled.diagnostics,
    });
  }
  return { family: true, id: meta.id, version: meta.version, topology: meta.topology, variants };
}

interface ResolvedVariantFields {
  readonly fields: VariantFields;
  readonly sourceAnchor: SourceAnchor | undefined;
}

async function resolveVariantFields(
  handle: VariantLeafHandle,
  seen: readonly string[] = [],
): Promise<ResolvedVariantFields> {
  const record = variantRecordOf(handle);
  if (record.kind === "definition") {
    return { fields: record.fields ?? {}, sourceAnchor: record.sourceAnchor };
  }
  const modulePath = record.modulePath;
  const sourceAnchor = record.sourceAnchor;
  if (modulePath === undefined || sourceAnchor === undefined) {
    throw new Error("variant.import: missing module path or source anchor");
  }
  if (seen.includes(modulePath)) {
    throw new Error(`variant.import: circular import chain (${[...seen, modulePath].join(" -> ")})`);
  }
  const resolvedUrl = new URL(modulePath, `file://${sourceAnchor.file}`).href;
  const imported = (await import(resolvedUrl)) as { readonly default?: unknown };
  const exported = imported.default;
  if (!isVariantHandle(exported)) {
    throw new Error(
      `variant.import(${JSON.stringify(modulePath)}): default export must be a variant(...) or ` +
        `variant.import(...) handle, not a plain object or other value`,
    );
  }
  return resolveVariantFields(exported, [...seen, modulePath]);
}
