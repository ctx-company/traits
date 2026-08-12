/**
 * The interop surface a consumer (the CDK) reads to recognize a
 * config-minted setting handle without depending on this package's
 * internals: a registered symbol (not module-unique) so a dual-instance
 * `node_modules` layout can't split identity, carrying the handle's ref and
 * compiled declaration.
 */
const SETTING_META = Symbol.for("ctx-traits.setting.meta");
/** Reads the interop meta a `setting.*` handle carries, or `undefined` for any other value. */
export function settingMetaOf(value) {
    return typeof value === "object" && value !== null
        ? value[SETTING_META]
        : undefined;
}
const settingMintListeners = [];
/**
 * Registers a listener invoked every time `setting.*` mints a handle — lets
 * the CDK fold a setting mint into its own trait-frame mint bookkeeping
 * (`recordTraitMint`) without this package depending on the CDK.
 */
export function onSettingMint(listener) {
    settingMintListeners.push(listener);
}
const SLUG_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
function validateSlug(value, fieldPath) {
    if (!SLUG_PATTERN.test(value)) {
        throw new Error(`${fieldPath}: expected lowercase slug, got ${JSON.stringify(value)}`);
    }
}
function compact(value) {
    return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined));
}
function settingOf(schema, fields) {
    validateSlug(fields.id, "setting.id");
    const declaration = compact({
        id: fields.id,
        schema,
        description: fields.description,
        default: fields.default,
        min: fields.min,
        max: fields.max,
    });
    const ref = `setting:${fields.id}`;
    const handle = {};
    Object.defineProperty(handle, SETTING_META, {
        value: { ref, declaration },
        enumerable: false,
        configurable: true,
    });
    settingMintListeners.forEach((listener) => listener(fields.id, ref, declaration));
    return handle;
}
export const setting = {
    number: (fields) => settingOf("number", fields),
    text: (fields) => settingOf("text", fields),
    boolean: (fields) => settingOf("boolean", fields),
};
