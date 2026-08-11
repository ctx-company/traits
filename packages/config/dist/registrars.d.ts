import type { AgentDefaults, GitTable, HarnessDefinition, HostOverride, MergeTable, ModelPricing, PublishTable, RegistryTable, RepoOverride, RoleAssignmentValue, RunTable, TasksTable, TraitDefaults, WorktreeConfig } from "./generated.js";
/** `[run]` — one per config build, second call is a named error. */
export declare function defineRun(fields: RunTable): void;
/** `[worktree]` — one per config build, second call is a named error. */
export declare function defineWorktree(fields: WorktreeConfig): void;
/** `[merge]` — one per config build, second call is a named error. */
export declare function defineMerge(fields: MergeTable): void;
/** `[git]` — one per config build, second call is a named error. */
export declare function defineGit(fields: GitTable): void;
/** `[tasks]` — one per config build, second call is a named error. */
export declare function defineTasks(fields: TasksTable): void;
/**
 * `[agent]` non-role fields — `agent.variant` (`VariantOverride`) has no
 * other authoring path. Roles go through {@link defineRole} exclusively —
 * `role` is omitted from the parameter type so there is exactly one
 * authoring path for `[agent.role.*]`.
 */
export declare function defineAgentDefaults(fields: Omit<AgentDefaults, "role">): void;
/** `[publish]` — one per config build, second call is a named error. */
export declare function definePublish(fields: PublishTable): void;
/** `[registry]` — one per config build, second call is a named error. */
export declare function defineRegistry(fields: RegistryTable): void;
/**
 * `[agent.role.<role>]` — a single `ProfileAssignment`, or an array for a
 * multi-seat role (`RoleAssignmentValue = ProfileAssignment |
 * ProfileAssignment[]`). Duplicate `role` in one build is a named error.
 */
export declare function defineRole(role: string, value: RoleAssignmentValue): void;
/** `[pricing.<model>]` — duplicate `model` in one build is a named error. */
export declare function definePricing(model: string, fields: ModelPricing): void;
/** `[harness.<id>]` — duplicate `id` in one build is a named error. */
export declare function defineHarness(id: string, fields: HarnessDefinition): void;
/** `[host.<id>]` — duplicate `id` in one build is a named error. */
export declare function defineHost(id: string, fields: HostOverride): void;
/**
 * `[repo.<key>]` — duplicate `key` in one build is a named error. Only
 * legal in a user-global config build; `evaluateConfigFunction` rejects any
 * registered repo key when the build's layer is not `"user-global"`.
 */
export declare function defineRepo(key: string, fields: RepoOverride): void;
/**
 * `[trait.<traitId>]` — duplicate `traitId` in one build is a named error.
 * Shares its name with the CDK's `defineTrait` deliberately (owner ruling:
 * different package, different context; config.ts never imports the CDK).
 */
export declare function defineTrait(traitId: string, fields: TraitDefaults): void;
//# sourceMappingURL=registrars.d.ts.map