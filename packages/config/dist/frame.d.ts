/**
 * The ambient config-build frame — mirrors the CDK trait-frame discipline
 * (`packages/cdk/src/functional/context.ts`): one frame open at a time,
 * `define*` registrars write into whichever frame is currently active, and
 * every registrar call outside a frame is a named build error. Implemented
 * locally rather than imported from the CDK — config.ts never imports the
 * CDK (owner ruling) — but the rules and error grammar are the same
 * discipline, not a second mechanism.
 */
export type ConfigTable = string;
export interface ConfigFrame {
    readonly singletons: Map<ConfigTable, unknown>;
    readonly keyed: Map<ConfigTable, Map<string, unknown>>;
    /** Read-and-cleared at `openConfigFrame`, so no run can leak into the next. */
    legacyDefineConfigSeen: boolean;
}
export declare function isConfigFrameActive(): boolean;
export declare function markLegacyDefineConfigTopLevel(): void;
export declare function openConfigFrame(): void;
/** Closes and returns the active config frame — throws if none is open. */
export declare function closeConfigFrame(): ConfigFrame;
/** The active config frame, or a named build error naming `caller` if none is open — every `define*` registrar calls this first. */
export declare function currentConfigFrame(caller: string): ConfigFrame;
/** Singleton registrar helper: the second call for `table` in one build is a named error. */
export declare function registerSingleton(caller: string, table: ConfigTable, value: unknown): void;
/** Keyed registrar helper: a duplicate `key` for `table` in one build is a named error. */
export declare function registerKeyed(caller: string, table: ConfigTable, key: string, value: unknown): void;
//# sourceMappingURL=frame.d.ts.map