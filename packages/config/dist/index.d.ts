import type { CtxConfig, ProfileAssignment } from "./generated.js";
export * from "./generated.js";
/**
 * Identity + type anchor: TypeScript inference and editor completion are the
 * whole point, `config.ts` never evaluates at load time — `ctx traits config
 * build` compiles it into `config.toml` once, ahead of any command that
 * reads config.
 */
export declare function defineConfig(config: CtxConfig): CtxConfig;
/** Type-anchored builder for one role's `[agent.role.<role>]` assignment. */
export declare function assignment(value: ProfileAssignment): ProfileAssignment;
//# sourceMappingURL=index.d.ts.map