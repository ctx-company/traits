export interface CtxConfig {
    agent?: AgentDefaults;
    git?: GitTable;
    harness?: Record<string, HarnessDefinition>;
    host?: Record<string, HostOverride>;
    merge?: MergeTable;
    pricing?: Record<string, ModelPricing>;
    publish?: PublishTable;
    registry?: RegistryTable;
    repo?: Record<string, RepoOverride>;
    run?: RunTable;
    schemaVersion?: string;
    tasks?: TasksTable;
    trait?: Record<string, TraitDefaults>;
    worktree?: WorktreeConfig;
}
export interface AgentDefaults {
    modelTier?: Record<string, ProfileAssignment>;
    role?: Record<string, RoleAssignmentValue>;
    variant?: Record<string, VariantOverride>;
}
export type AgentModelTier = "top" | "fast";
export type AutoClosePolicy = "confirm" | "checked" | "merge";
export type BillingMode = "subscription" | "api";
export interface BuildCacheConfig {
    env: string;
}
export interface GeneratedArtifact {
    paths: string[];
    rebuild: string[][];
}
export interface GitTable {
    longSeconds?: number;
}
export interface HarnessCliConvention {
    argv?: string[];
    dirFlag?: string;
    jsonSchemaFlag?: string;
    modelFlag?: string;
    narratorArgv?: string[];
    output?: string;
    promptVia?: string;
    reasoningEffortFlag?: string;
    resumeFlag?: string;
    sessionFlag?: string;
    stream?: boolean;
    systemPromptFlag?: string;
    warmArgv?: string[];
}
export interface HarnessDefinition {
    billing?: BillingMode;
    bin?: string;
    cli?: HarnessCliConvention;
    kind?: string;
    mcp?: HarnessMcpConvention;
    transports?: RunTransport[];
    versionProbe?: string[];
}
export interface HarnessMcpConvention {
    allowedTools?: string[];
    allowedToolsFlag?: string;
    configVia?: string;
    mcpConfigFlag?: string;
    reasoningEffortFlag?: string;
    systemPromptFlag?: string;
}
export interface HostOverride {
    format?: string;
    globalPath?: string;
    profile?: string;
    projectPath?: string;
}
export type MergeOverlap = "land" | "park";
export interface MergeTable {
    auto?: boolean;
    branch?: string;
    deep?: boolean;
    diskFloorMb?: number;
    gate?: string[][];
    gateSeconds?: number;
    generated?: GeneratedArtifact[];
    overlap?: MergeOverlap;
    retryAttempts?: number;
    retryBackoffMs?: number;
    wait?: boolean;
}
export interface ModelPricing {
    usdPerMtok?: number;
}
export interface PortDefaults {
    port?: Record<string, string>;
}
export interface ProfileAssignment {
    apiKeyEnv?: string;
    baseUrl?: string;
    budget?: RoleBudget;
    connectTimeoutMs?: number;
    count?: number;
    extraArgs?: string[];
    harness?: string;
    mode?: RunAssignmentMode;
    model?: string;
    modelTier?: AgentModelTier;
    readTimeoutMs?: number;
    reasoningEffort?: string;
    retries?: number;
    sessionMode?: RunSessionMode;
    systemPrompt?: string;
    transport?: RunTransport;
    wire?: ProviderWire;
}
export type ProviderWire = "openai-compat" | "anthropic";
export interface PublishTable {
    exclude?: string[];
}
export interface RegistryTable {
    base?: string;
}
export interface RepoGitOverride {
    longSeconds?: number;
}
export interface RepoMergeOverride {
    auto?: boolean;
    deep?: boolean;
    wait?: boolean;
}
export interface RepoOverride {
    agent?: AgentDefaults;
    git?: RepoGitOverride;
    harness?: Record<string, HarnessDefinition>;
    host?: Record<string, HostOverride>;
    merge?: RepoMergeOverride;
    publish?: RepoPublishOverride;
    registry?: RepoRegistryOverride;
    run?: RepoRunOverride;
    worktree?: RepoWorktreeOverride;
}
export interface RepoPublishOverride {
    exclude?: string[];
}
export interface RepoRegistryOverride {
    base?: string;
}
export interface RepoRunOverride {
    buildCache?: Record<string, BuildCacheConfig>;
    story?: StoryLevel;
    wait?: boolean;
}
export interface RepoTripwireOverride {
    sentinel?: string[];
}
export interface RepoWorktreeOverride {
    env?: Record<string, string>;
    seed?: string[];
    tripwire?: RepoTripwireOverride;
    warm?: string[];
}
export type RoleAssignmentValue = ProfileAssignment | ProfileAssignment[];
export interface RoleBudget {
    frameSeconds?: number;
    idleSeconds?: number;
    maxRetries?: number;
    maxTokens?: number;
}
export type RunAssignmentMode = "harness" | "attach";
export type RunSessionMode = "per-frame" | "persistent";
export interface RunTable {
    attachWaitSeconds?: number;
    buildCache?: Record<string, BuildCacheConfig>;
    commandIdleSeconds?: number;
    commandSeconds?: number;
    frameSeconds?: number;
    idleSeconds?: number;
    inlinePromptBytes?: number;
    maxCostUsd?: number;
    maxFrames?: number;
    maxInFlight?: number;
    maxRetries?: number;
    maxTokens?: number;
    story?: StoryLevel;
    strictLoops?: boolean;
    totalSeconds?: number;
    usageWarningThreshold?: number;
    wait?: boolean;
    worktree?: boolean;
}
export type RunTransport = "cli" | "mcp" | "api";
export type StoryLevel = "default" | "detailed" | "assisted";
export interface TasksTable {
    autoClose?: AutoClosePolicy;
    dispatchTrait?: string;
}
export interface TraitDefaults {
    agent?: AgentDefaults;
    defaults?: PortDefaults;
    variant?: Record<string, TraitVariantDefaults>;
}
export interface TraitVariantDefaults {
    agent?: AgentDefaults;
}
export type TripwirePolicy = "park" | "warn";
export interface VariantOverride {
    role?: Record<string, RoleAssignmentValue>;
}
export interface WorktreeConfig {
    confinement?: WorktreeConfinementConfig;
    env?: Record<string, string>;
    retention?: WorktreeRetentionConfig;
    seed?: string[];
    setup?: string[][];
    setupCaptureBytes?: number;
    setupSeconds?: number;
    tripwire?: WorktreeTripwireConfig;
    warm?: string[];
}
export interface WorktreeConfinementConfig {
    allow?: string[];
    enabled?: boolean;
    sandbox?: boolean;
}
export interface WorktreeRetentionConfig {
    cheap?: string[];
    expensive?: string[];
    expensiveGraceDays?: number;
}
export interface WorktreeTripwireConfig {
    policy?: TripwirePolicy;
    sentinel?: string[];
}
//# sourceMappingURL=generated.d.ts.map