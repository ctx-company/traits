declare const SETTING_HANDLE_BRAND: unique symbol;
/**
 * A typed operator knob: declared in trait source, resolved from config
 * layers at activation. Accepted anywhere a `SlotHandle`/`PortHandle` is —
 * prompt interpolation, argv tokens, condition operands, structural fields
 * (e.g. the loop bound) — type-checked the same way (`@ctx-traits/cdk`
 * re-exports this type; the CDK never mints one itself, see 0180).
 */
export type SettingHandle<Value = unknown> = {
    readonly [SETTING_HANDLE_BRAND]?: Value;
};
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
export interface SettingMeta {
    readonly ref: string;
    readonly declaration: SettingDeclaration;
}
/** Reads the interop meta a `setting.*` handle carries, or `undefined` for any other value. */
export declare function settingMetaOf(value: unknown): SettingMeta | undefined;
type SettingMintListener = (id: string, ref: string, declaration: SettingDeclaration) => void;
/**
 * Registers a listener invoked every time `setting.*` mints a handle — lets
 * the CDK fold a setting mint into its own trait-frame mint bookkeeping
 * (`recordTraitMint`) without this package depending on the CDK.
 */
export declare function onSettingMint(listener: SettingMintListener): void;
export declare const setting: SettingFunction;
export {};
//# sourceMappingURL=setting.d.ts.map