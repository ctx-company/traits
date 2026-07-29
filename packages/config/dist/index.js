export * from "./generated.js";
/**
 * Identity + type anchor: TypeScript inference and editor completion are the
 * whole point, `config.ts` never evaluates at load time — `ctx traits config
 * build` compiles it into `config.toml` once, ahead of any command that
 * reads config.
 */
export function defineConfig(config) {
    return config;
}
/** Type-anchored builder for one role's `[agent.role.<role>]` assignment. */
export function assignment(value) {
    return value;
}
