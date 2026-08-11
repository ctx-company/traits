/**
 * The ambient config-build frame — mirrors the CDK trait-frame discipline
 * (`packages/cdk/src/functional/context.ts`): one frame open at a time,
 * `define*` registrars write into whichever frame is currently active, and
 * every registrar call outside a frame is a named build error. Implemented
 * locally rather than imported from the CDK — config.ts never imports the
 * CDK (owner ruling) — but the rules and error grammar are the same
 * discipline, not a second mechanism.
 */
let activeConfigFrame;
/**
 * Set by `defineConfig` (see `index.ts`) when it is called with no config
 * frame active — a module-top-level `defineConfig({...})` call, which
 * `openConfigFrame` cannot otherwise see since the frame does not exist yet
 * at that point in module evaluation. Read-and-cleared at `openConfigFrame`
 * so no run can leak it into the next.
 */
let legacyDefineConfigTopLevelSeen = false;
export function isConfigFrameActive() {
    return activeConfigFrame !== undefined;
}
export function markLegacyDefineConfigTopLevel() {
    legacyDefineConfigTopLevelSeen = true;
}
export function openConfigFrame() {
    if (activeConfigFrame !== undefined) {
        throw new Error("evaluateConfigFunction: a config function is already being evaluated — one at a time");
    }
    const legacyDefineConfigSeen = legacyDefineConfigTopLevelSeen;
    legacyDefineConfigTopLevelSeen = false;
    activeConfigFrame = {
        singletons: new Map(),
        keyed: new Map(),
        legacyDefineConfigSeen,
    };
}
/** Closes and returns the active config frame — throws if none is open. */
export function closeConfigFrame() {
    const frame = activeConfigFrame;
    if (frame === undefined)
        throw new Error("evaluateConfigFunction: no active config function to close");
    activeConfigFrame = undefined;
    return frame;
}
/** The active config frame, or a named build error naming `caller` if none is open — every `define*` registrar calls this first. */
export function currentConfigFrame(caller) {
    if (activeConfigFrame === undefined) {
        throw new Error(`${caller} called outside the config build — config.ts must default-export a function of define* calls`);
    }
    return activeConfigFrame;
}
/** Singleton registrar helper: the second call for `table` in one build is a named error. */
export function registerSingleton(caller, table, value) {
    const frame = currentConfigFrame(caller);
    if (frame.singletons.has(table)) {
        throw new Error(`${caller} was already called in this config build — only one ${caller} call is allowed`);
    }
    frame.singletons.set(table, value);
}
/** Keyed registrar helper: a duplicate `key` for `table` in one build is a named error. */
export function registerKeyed(caller, table, key, value) {
    const frame = currentConfigFrame(caller);
    let byKey = frame.keyed.get(table);
    if (byKey === undefined) {
        byKey = new Map();
        frame.keyed.set(table, byKey);
    }
    if (byKey.has(key)) {
        throw new Error(`${caller}(${JSON.stringify(key)}, ...) was already called with this key in this config build`);
    }
    byKey.set(key, value);
}
