import { closeConfigFrame, openConfigFrame } from "./frame.js";
const SINGLETON_TABLES = [
    "budget",
    "drive",
    "worktree",
    "merge",
    "git",
    "tasks",
    "agent",
    "publish",
    "registry",
    "preferences",
];
const KEYED_TABLES = ["pricing", "harness", "host", "repo", "trait"];
function assembleKeyed(frame, table) {
    const byKey = frame.keyed.get(table);
    if (byKey === undefined || byKey.size === 0)
        return undefined;
    return Object.fromEntries(byKey);
}
/**
 * Evaluates a hook-style config default export — a function of `define*`
 * calls — into a `CtxConfig` document. Opens the ambient config frame,
 * calls `fn()` synchronously (an async default export is a named error,
 * same rule the CDK enforces for trait functions), closes the frame in
 * `finally`, then assembles the collected tables. Only called from the
 * `ctx traits config build` emit script — nothing on any load path calls
 * this.
 *
 * A `defineConfig({...})` call reached while this frame is open is the
 * mixed-form error — `defineConfig` (see `index.ts`) throws it directly, so
 * this function never needs to detect it after the fact.
 */
export function evaluateConfigFunction(fn, opts = {}) {
    openConfigFrame();
    let frame;
    try {
        const result = fn();
        if (result !== undefined && result !== null && typeof result.then === "function") {
            throw new Error("evaluateConfigFunction: the config default export returned a promise — config.ts must be synchronous");
        }
    }
    finally {
        frame = closeConfigFrame();
    }
    if (frame.legacyDefineConfigSeen) {
        throw new Error("config.ts mixes defineConfig({...}) with define* registrar calls — pick one authoring form, not both");
    }
    if (opts.layer !== "user-global") {
        const repoKeys = frame.keyed.get("repo");
        if (repoKeys !== undefined && repoKeys.size > 0) {
            const [firstKey] = repoKeys.keys();
            throw new Error(`defineRepo(${JSON.stringify(firstKey)}, ...) is only allowed in the user-global config — [repo.*] is rejected outside the user-global layer`);
        }
        if (frame.singletons.has("preferences")) {
            throw new Error("definePreferences(...) is only allowed in the user-global config — [preferences] is rejected outside the user-global layer");
        }
    }
    const config = {};
    for (const table of SINGLETON_TABLES) {
        const value = frame.singletons.get(table);
        if (value !== undefined)
            config[table] = value;
    }
    for (const table of KEYED_TABLES) {
        const assembled = assembleKeyed(frame, table);
        if (assembled !== undefined)
            config[table] = assembled;
    }
    const roles = assembleKeyed(frame, "agent.role");
    if (roles !== undefined) {
        const agent = config.agent ?? {};
        agent.role = roles;
        config.agent = agent;
    }
    return config;
}
