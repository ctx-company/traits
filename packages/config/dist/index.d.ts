import type { CtxConfig, ProfileAssignment } from "./generated.js";
export * from "./evaluate.js";
export * from "./generated.js";
export * from "./registrars.js";
export { onSettingMint, setting, settingMetaOf } from "./setting.js";
export type { SettingDeclaration, SettingFunction, SettingHandle, SettingMeta } from "./setting.js";
/**
 * The object-literal authoring form — kept working through a deprecation
 * window. Prefer the hook form: a default-export function of `define*`
 * registrar calls, evaluated by `evaluateConfigFunction` (used internally
 * by `ctx traits config build`). Identity + type anchor when used at the
 * top level: TypeScript inference and editor completion are the whole
 * point, `config.ts` never evaluates at load time.
 *
 * @deprecated Prefer the hook form — a default-export function calling
 * `define*` registrars (`defineRun`, `defineRole`, ...) — over
 * `defineConfig({...})`.
 *
 * Calling this alongside `define*` registrars in the same file — whether
 * inside the default-export function or at module top level — is a named
 * mixed-form error; pick one authoring form per file.
 */
export declare function defineConfig(config: CtxConfig): CtxConfig;
/** Type-anchored builder for one role's `[agent.role.<role>]` assignment. */
export declare function assignment(value: ProfileAssignment): ProfileAssignment;
//# sourceMappingURL=index.d.ts.map