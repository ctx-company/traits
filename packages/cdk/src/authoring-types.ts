/**
 * Type-only leaf module (P482 §4.4): authoring-side types shared by
 * `schema.ts`, `variant.ts`, and `meta.ts` that would otherwise force an
 * import cycle between them. Every export here is `import type`-only from
 * `generated.js`/`handles.js` except `VARIANT_HANDLE`, which must be a real
 * runtime value because `variant.ts` constructs objects keyed by it.
 */
import type { SchemaHandle, SchemaRef } from "./handles.js";

/** A schema field/slot value: a built-in ref string or a declared schema handle. */
export type SchemaValue<Value = unknown> = SchemaRef<Value> | SchemaHandle<Value>;

/** Runtime brand key identifying a variant leaf handle. Not type-only — `variant.ts` builds objects keyed by it. */
export const VARIANT_HANDLE: unique symbol = Symbol("ctx.traits.cdk.variant");
export interface VariantHandleBase {
  readonly [VARIANT_HANDLE]: string;
}
/** A complete variant leaf definition, authored inline via `variant({...})`. */
export type VariantHandle = VariantHandleBase & { readonly default: () => VariantHandle; };
/** A variant leaf resolved from another module via `variant.import(path)`. */
export type VariantImportHandle = VariantHandleBase & { readonly default: () => VariantImportHandle; };
export type VariantLeafHandle = VariantHandle | VariantImportHandle;
/**
 * A (possibly nested) `variants` map: leaves keyed by segment, or nested
 * maps for a deeper level — joined with `:` when flattened (`strict:sub`).
 */
export type TraitFamilyMap = { readonly [segment: string]: VariantLeafHandle | TraitFamilyMap; };

/** One resolved leaf: its colon-joined path and the variant definition it resolved to. */
export interface FlattenedVariantLeaf {
  readonly path: string;
  readonly segments: readonly string[];
  readonly handle: VariantLeafHandle;
}

/** The recursive variant topology a package manifest records: each level's default and each leaf's path. */
export type VariantTopologyNode =
  | { readonly kind: "leaf"; readonly path: string; }
  | {
    readonly kind: "map";
    readonly default: string;
    readonly children: Readonly<Record<string, VariantTopologyNode>>;
  };
