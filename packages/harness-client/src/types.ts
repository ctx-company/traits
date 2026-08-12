/** Host facts collected before the model sees the message. Only facts the host actually exposes. */
export interface HostFacts {
  readonly task: string;
  readonly files: readonly string[];
  readonly mode?: string;
  readonly languages: readonly string[];
  readonly session?: string;
  readonly explicitInvocation?: string;
  readonly traitId?: string;
}

/** Mirrors `ctx_traits_core::resolve::Candidate`, kebab-case JSON from `ctx traits resolve --json`. */
export interface ResolveCandidate {
  readonly "trait-id": string;
  readonly version: string;
  readonly active: boolean;
  readonly "load-level": string;
  readonly "estimated-tokens": number;
  readonly priority: number;
  readonly score: number;
  readonly "budget-decision": string;
  readonly "estimate-source": string;
  readonly "reason-codes"?: readonly string[];
}

/** Mirrors `ctx_traits_core::resolve::Response`, the JSON shape of `ctx traits resolve --json`. */
export interface ResolveResponse {
  readonly selected?: readonly ResolveCandidate[];
  readonly rejected?: readonly ResolveCandidate[];
}

/** Mirrors the `PromptJsonOutput` shape of `ctx traits prompt <id> --json`. */
export interface PromptResponse {
  readonly text: string;
  readonly digests: {
    readonly source: string;
    readonly canonical: string;
    readonly "resource-manifest"?: string | null;
    readonly "model-view": string;
  };
}

export interface InjectedTrait {
  readonly traitId: string;
  readonly digest: string;
  readonly text: string;
}

/** Mirrors `ContextPlanRowReport`, one row of `ctx traits context plan --json`. */
export interface ContextPlanRow {
  readonly "trait-id": string;
  readonly "model-view-digest": string;
  readonly action: string;
  readonly "stale-reason"?: string;
  readonly text: string;
}

/** Mirrors `ContextPlanReport`, the `value` payload of `ctx traits context plan --json`. */
export interface ContextPlanValue {
  readonly host: string;
  readonly "host-session": string;
  readonly committed: boolean;
  readonly rows: readonly ContextPlanRow[];
  readonly note: string;
}

/** Mirrors `core::response::ResponseError`. */
export interface EnvelopeError {
  readonly code: string;
  readonly message: string;
  readonly details?: Record<string, string>;
}

/** Mirrors `core::response::Warning`. */
export interface Warning {
  readonly code: string;
  readonly message: string;
  readonly path: string | null;
}

/** Mirrors `core::response::CapabilityReport`. */
export interface CapabilityReport {
  readonly capability: string;
  readonly supported: boolean;
  readonly reason: string | null;
}

/**
 * Mirrors `core::response::Envelope<T>`, the JSON ABI envelope shape used by
 * `ctx traits context plan --json` (unlike `resolve --json` / `prompt --json`,
 * which are bare objects with no envelope).
 */
export interface Envelope<T> {
  readonly "schema-version": string;
  readonly ok: boolean;
  readonly value?: T;
  readonly error?: EnvelopeError;
  readonly warnings: readonly Warning[];
  readonly capabilities: readonly CapabilityReport[];
}

// JSON grammar aliases, matching `packages/cdk/src/generated.ts` verbatim. Not part of the
// package's public surface — index.ts imports them but does not re-export.
export type JsonValue = null | boolean | number | string | readonly JsonValue[] | JsonObject;
export type JsonObject = { readonly [key: string]: JsonValue | undefined };
