declare const SETTING_HANDLE_BRAND: unique symbol;

/**
 * A typed operator knob: declared in trait source, resolved from config
 * layers at activation. Accepted anywhere a `SlotHandle`/`PortHandle` is —
 * prompt interpolation, argv tokens, condition operands, structural fields
 * (e.g. the loop bound) — type-checked the same way (`@ctx-traits/cdk`
 * re-exports this type; the CDK never mints one itself, see 0180).
 */
export type SettingHandle<Value = unknown> = { readonly [SETTING_HANDLE_BRAND]?: Value };

interface SettingFields<Value> {
  readonly id: string;
  readonly description: string;
  readonly default: Value;
}

interface NumberSettingFields extends SettingFields<number> {
  readonly min?: number;
  readonly max?: number;
}

/** The canonical `[[setting]]` declaration shape a `setting.*` call compiles to. */
export interface SettingDeclaration {
  readonly id: string;
  readonly schema: "number" | "text" | "boolean";
  readonly description: string;
  readonly default: unknown;
  readonly min?: number;
  readonly max?: number;
}

/**
 * Declares a typed operator knob: a `[[setting]]` resolved at ACTIVATION
 * from config layers (canonical default < global config < repo config),
 * never at build time. Accepted anywhere a slot/port input is — prompt
 * interpolation, `input.command` argv tokens, condition comparisons,
 * include lists, structural fields (e.g. `loop.maxIterations`) — the same
 * way a `SlotHandle`/`PortHandle` is.
 *
 * @example
 * ```ts
 * const rounds = setting.number({
 *   id: "review-rounds",
 *   description: "Number of reviewer loop iterations.",
 *   default: 3,
 *   min: 1,
 *   max: 10,
 * });
 * loop.maxIterations(rounds);
 * ```
 */
export interface SettingFunction {
  number(fields: NumberSettingFields): SettingHandle<number>;
  text(fields: SettingFields<string>): SettingHandle<string>;
  boolean(fields: SettingFields<boolean>): SettingHandle<boolean>;
}

/**
 * The interop surface a consumer (the CDK) reads to recognize a
 * config-minted setting handle without depending on this package's
 * internals: a registered symbol (not module-unique) so a dual-instance
 * `node_modules` layout can't split identity, carrying the handle's ref and
 * compiled declaration.
 */
const SETTING_META: unique symbol = Symbol.for("ctx-traits.setting.meta");

export interface SettingMeta {
  readonly ref: string;
  readonly declaration: SettingDeclaration;
}

/** Reads the interop meta a `setting.*` handle carries, or `undefined` for any other value. */
export function settingMetaOf(value: unknown): SettingMeta | undefined {
  return typeof value === "object" && value !== null
    ? (value as { readonly [SETTING_META]?: SettingMeta })[SETTING_META]
    : undefined;
}

type SettingMintListener = (id: string, ref: string, declaration: SettingDeclaration) => void;
const settingMintListeners: SettingMintListener[] = [];

/**
 * Registers a listener invoked every time `setting.*` mints a handle — lets
 * the CDK fold a setting mint into its own trait-frame mint bookkeeping
 * (`recordTraitMint`) without this package depending on the CDK.
 */
export function onSettingMint(listener: SettingMintListener): void {
  settingMintListeners.push(listener);
}

const SLUG_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

function validateSlug(value: string, fieldPath: string): void {
  if (!SLUG_PATTERN.test(value)) {
    throw new Error(`${fieldPath}: expected lowercase slug, got ${JSON.stringify(value)}`);
  }
}

function compact<T extends Record<string, unknown>>(value: T): T {
  return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined)) as T;
}

function settingOf<Value>(
  schema: "number" | "text" | "boolean",
  fields: SettingFields<Value> & { readonly min?: number; readonly max?: number },
): SettingHandle<Value> {
  validateSlug(fields.id, "setting.id");
  const declaration = compact({
    id: fields.id,
    schema,
    description: fields.description,
    default: fields.default,
    min: fields.min,
    max: fields.max,
  }) as SettingDeclaration;
  const ref = `setting:${fields.id}`;
  const handle: SettingHandle<Value> = {};
  Object.defineProperty(handle, SETTING_META, {
    value: { ref, declaration },
    enumerable: false,
    configurable: true,
  });
  settingMintListeners.forEach((listener) => listener(fields.id, ref, declaration));
  return handle;
}

export const setting: SettingFunction = {
  number: (fields) => settingOf<number>("number", fields),
  text: (fields) => settingOf<string>("text", fields),
  boolean: (fields) => settingOf<boolean>("boolean", fields),
};
