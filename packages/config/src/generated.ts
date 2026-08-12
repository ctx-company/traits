// GENERATED FILE — do not edit by hand.
// Regenerate with `ctx traits sdk-generate`.

export type JsonValue = null | boolean | number | string | readonly JsonValue[] | { readonly [key: string]: JsonValue | undefined };

export interface CtxConfig {
  agent?: AgentDefaults;
  budget?: RunProfileBudget;
  drive?: DriveTable;
  git?: GitTable;
  harness?: Record<string, HarnessDefinition>;
  host?: Record<string, HostOverride>;
  merge?: MergeTable;
  preferences?: PreferencesTable;
  pricing?: Record<string, ModelPricing>;
  publish?: PublishTable;
  registry?: RegistryTable;
  repo?: Record<string, RepoOverride>;
  schemaVersion?: string;
  tasks?: TasksTable;
  trait?: Record<string, TraitDefaults>;
  worktree?: WorktreeConfig;
}

export interface AgentDefaults {
  role?: Record<string, RoleAssignmentValue>;
  variant?: Record<string, VariantOverride>;
}

export type AutoClosePolicy = "confirm" | "checked" | "merge";

export type BillingMode = "subscription" | "api";

export interface BuildCacheConfig {
  env: string;
}

export type ConfigFormatPreference = "toml" | "ts";

export interface DriveTable {
  inlinePromptBytes?: number;
  maxInFlight?: number;
  story?: StoryLevel;
  strictLoops?: boolean;
  usageWarningThreshold?: number;
  wait?: boolean;
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

export interface PreferencesTable {
  configFormat?: ConfigFormatPreference;
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

export interface RepoDriveOverride {
  story?: StoryLevel;
  wait?: boolean;
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
  drive?: RepoDriveOverride;
  git?: RepoGitOverride;
  harness?: Record<string, HarnessDefinition>;
  host?: Record<string, HostOverride>;
  merge?: RepoMergeOverride;
  publish?: RepoPublishOverride;
  registry?: RepoRegistryOverride;
  worktree?: RepoWorktreeOverride;
}

export interface RepoPublishOverride {
  exclude?: string[];
}

export interface RepoRegistryOverride {
  base?: string;
}

export interface RepoTripwireOverride {
  sentinel?: string[];
}

export interface RepoWorktreeOverride {
  buildCache?: Record<string, BuildCacheConfig>;
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

export interface RunProfileBudget {
  attachWaitSeconds?: number;
  commandIdleSeconds?: number;
  commandSeconds?: number;
  frameSeconds?: number;
  idleSeconds?: number;
  maxCostUsd?: number;
  maxFrames?: number;
  maxRetries?: number;
  maxTokens?: number;
  totalSeconds?: number;
}

export type RunSessionMode = "per-frame" | "persistent";

export type RunTransport = "cli" | "mcp" | "api";

export type StoryLevel = "default" | "detailed" | "assisted";

export interface TasksTable {
  autoClose?: AutoClosePolicy;
  dispatchTrait?: string;
}

export interface TraitDefaults {
  agent?: AgentDefaults;
  budget?: RunProfileBudget;
  defaults?: PortDefaults;
  setting?: Record<string, JsonValue>;
  variant?: Record<string, TraitVariantDefaults>;
}

export interface TraitVariantDefaults {
  agent?: AgentDefaults;
  budget?: RunProfileBudget;
  setting?: Record<string, JsonValue>;
}

export type TripwirePolicy = "park" | "warn";

export interface VariantOverride {
  role?: Record<string, RoleAssignmentValue>;
}

export interface WorktreeConfig {
  buildCache?: Record<string, BuildCacheConfig>;
  confinement?: WorktreeConfinementConfig;
  enabled?: boolean;
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

