import { registerKeyed, registerSingleton } from "./frame.js";
import type {
  AgentDefaults,
  DriveTable,
  GitTable,
  HarnessDefinition,
  HostOverride,
  MergeTable,
  ModelPricing,
  PublishTable,
  RegistryTable,
  RepoOverride,
  RoleAssignmentValue,
  RunProfileBudget,
  TasksTable,
  TraitDefaults,
  WorktreeConfig,
} from "./generated.js";

/** `[budget]` — one per config build, second call is a named error. */
export function defineBudget(fields: RunProfileBudget): void {
  registerSingleton("defineBudget", "budget", fields);
}

/** `[drive]` — one per config build, second call is a named error. */
export function defineDrive(fields: DriveTable): void {
  registerSingleton("defineDrive", "drive", fields);
}

/** `[worktree]` — one per config build, second call is a named error. */
export function defineWorktree(fields: WorktreeConfig): void {
  registerSingleton("defineWorktree", "worktree", fields);
}

/** `[merge]` — one per config build, second call is a named error. */
export function defineMerge(fields: MergeTable): void {
  registerSingleton("defineMerge", "merge", fields);
}

/** `[git]` — one per config build, second call is a named error. */
export function defineGit(fields: GitTable): void {
  registerSingleton("defineGit", "git", fields);
}

/** `[tasks]` — one per config build, second call is a named error. */
export function defineTasks(fields: TasksTable): void {
  registerSingleton("defineTasks", "tasks", fields);
}

/**
 * `[agent]` non-role fields — `agent.variant` (`VariantOverride`) has no
 * other authoring path. Roles go through {@link defineRole} exclusively —
 * `role` is omitted from the parameter type so there is exactly one
 * authoring path for `[agent.role.*]`.
 */
export function defineAgentDefaults(fields: Omit<AgentDefaults, "role">): void {
  registerSingleton("defineAgentDefaults", "agent", fields);
}

/** `[publish]` — one per config build, second call is a named error. */
export function definePublish(fields: PublishTable): void {
  registerSingleton("definePublish", "publish", fields);
}

/** `[registry]` — one per config build, second call is a named error. */
export function defineRegistry(fields: RegistryTable): void {
  registerSingleton("defineRegistry", "registry", fields);
}

/**
 * `[agent.role.<role>]` — a single `ProfileAssignment`, or an array for a
 * multi-seat role (`RoleAssignmentValue = ProfileAssignment |
 * ProfileAssignment[]`). Duplicate `role` in one build is a named error.
 */
export function defineRole(role: string, value: RoleAssignmentValue): void {
  registerKeyed("defineRole", "agent.role", role, value);
}

/** `[pricing.<model>]` — duplicate `model` in one build is a named error. */
export function definePricing(model: string, fields: ModelPricing): void {
  registerKeyed("definePricing", "pricing", model, fields);
}

/** `[harness.<id>]` — duplicate `id` in one build is a named error. */
export function defineHarness(id: string, fields: HarnessDefinition): void {
  registerKeyed("defineHarness", "harness", id, fields);
}

/** `[host.<id>]` — duplicate `id` in one build is a named error. */
export function defineHost(id: string, fields: HostOverride): void {
  registerKeyed("defineHost", "host", id, fields);
}

/**
 * `[repo.<key>]` — duplicate `key` in one build is a named error. Only
 * legal in a user-global config build; `evaluateConfigFunction` rejects any
 * registered repo key when the build's layer is not `"user-global"`.
 */
export function defineRepo(key: string, fields: RepoOverride): void {
  registerKeyed("defineRepo", "repo", key, fields);
}

/**
 * `[trait.<traitId>]` — duplicate `traitId` in one build is a named error.
 * Shares its name with the CDK's `defineTrait` deliberately (owner ruling:
 * different package, different context; config.ts never imports the CDK).
 */
export function defineTrait(traitId: string, fields: TraitDefaults): void {
  registerKeyed("defineTrait", "trait", traitId, fields);
}
