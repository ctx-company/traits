import { execFile } from "node:child_process";

import type {
  ContextPlanRow,
  ContextPlanValue,
  HostFacts,
  InjectedTrait,
  JsonObject,
  JsonValue,
  PromptResponse,
  ResolveCandidate,
  ResolveResponse,
} from "./types.js";

export type {
  ContextPlanRow,
  ContextPlanValue,
  Envelope,
  EnvelopeError,
  HostFacts,
  InjectedTrait,
  PromptResponse,
  ResolveCandidate,
  ResolveResponse,
} from "./types.js";

/** Thrown for any failure this adapter must surface explicitly: never a silent no-op. */
export class CtxCliError extends Error {}

/** One sticky-union entry: what a Family-B adapter (opencode, pi) re-asserts every turn. */
export interface UnionEntry {
  readonly digest: string;
  readonly text: string;
}

/**
 * The wire-facing provenance label the model sees, and the pointer a user
 * follows to reproduce an activation decision — shared verbatim by every
 * Family-B adapter so it can only ever drift in one place.
 */
export function provenanceBlock(traitId: string, entry: UnionEntry): string {
  // Illustrative pointer, not a directly runnable command: `ctx traits explain` requires
  // `--task <TASK>` (or `--scaffold`) to reproduce a specific activation decision.
  const provenance =
    `[ctx.traits] ${traitId} (model-view-digest: ${entry.digest}) — see \`ctx traits explain ${traitId} --task <task>\``;
  return `${provenance}\n${entry.text}`;
}

function runCtx(ctxPath: string, args: readonly string[], cwd: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(ctxPath, [...args], { cwd, maxBuffer: 64 * 1024 * 1024 }, (error, stdout, stderr) => {
      if (error) {
        reject(
          new CtxCliError(
            `ctx.traits: \`${ctxPath} ${args.join(" ")}\` failed: ${error.message}${
              stderr ? `\nstderr: ${stderr}` : ""
            }`,
          ),
        );
        return;
      }
      resolve(stdout);
    });
  });
}

function parseJson(raw: string, context: string): JsonValue {
  try {
    return JSON.parse(raw) as JsonValue;
  } catch (cause) {
    throw new CtxCliError(`ctx.traits: malformed JSON from ${context}: ${(cause as Error).message}`);
  }
}

function isRecord(value: JsonValue | undefined): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Every field `ctx_traits_core::resolve::Candidate` mirrors — see {@link ResolveCandidate}. */
function isResolveCandidate(value: JsonValue): value is ResolveCandidate & JsonObject {
  return isRecord(value)
    && typeof value["trait-id"] === "string"
    && typeof value["version"] === "string"
    && typeof value["active"] === "boolean"
    && typeof value["load-level"] === "string"
    && typeof value["estimated-tokens"] === "number"
    && typeof value["priority"] === "number"
    && typeof value["score"] === "number"
    && typeof value["budget-decision"] === "string"
    && typeof value["estimate-source"] === "string"
    && (value["reason-codes"] === undefined
      || (Array.isArray(value["reason-codes"]) && value["reason-codes"].every((code) => typeof code === "string")));
}

/**
 * Full structural check for `ctx traits resolve --json`'s response shape. A
 * structurally-incompatible response (e.g. a CLI version skew, or a non-JSON error
 * object) must never silently become "no selection" — it must throw.
 */
function asResolveResponse(raw: JsonValue, context: string): ResolveResponse {
  if (!isRecord(raw)) {
    throw new CtxCliError(`ctx.traits: malformed response shape from ${context}: expected a JSON object`);
  }
  if (raw["selected"] !== undefined && !Array.isArray(raw["selected"])) {
    throw new CtxCliError(`ctx.traits: malformed response shape from ${context}: "selected" is not an array`);
  }
  if (raw["rejected"] !== undefined && !Array.isArray(raw["rejected"])) {
    throw new CtxCliError(`ctx.traits: malformed response shape from ${context}: "rejected" is not an array`);
  }
  if (!isResolveResponse(raw)) {
    throw new CtxCliError(
      `ctx.traits: malformed response shape from ${context}: a "selected"/"rejected" entry does not match ResolveCandidate`,
    );
  }
  return raw;
}

function isResolveResponse(value: JsonObject): value is JsonObject & ResolveResponse {
  return (value["selected"] === undefined
    || (Array.isArray(value["selected"]) && value["selected"].every(isResolveCandidate)))
    && (value["rejected"] === undefined
      || (Array.isArray(value["rejected"]) && value["rejected"].every(isResolveCandidate)));
}

/** Full structural check for `ctx traits prompt <id> --json`'s response shape. */
function asPromptResponse(raw: JsonValue, context: string): PromptResponse {
  if (!isRecord(raw)) {
    throw new CtxCliError(`ctx.traits: malformed response shape from ${context}: expected a JSON object`);
  }
  if (typeof raw["text"] !== "string") {
    throw new CtxCliError(`ctx.traits: malformed response shape from ${context}: "text" is not a string`);
  }
  const digests = raw["digests"];
  if (
    !isRecord(digests) || typeof digests["source"] !== "string" || typeof digests["canonical"] !== "string"
    || typeof digests["model-view"] !== "string"
    || (digests["resource-manifest"] !== undefined && digests["resource-manifest"] !== null
      && typeof digests["resource-manifest"] !== "string")
  ) {
    throw new CtxCliError(`ctx.traits: malformed response shape from ${context}: "digests" is not well-formed`);
  }
  return {
    text: raw["text"],
    digests: {
      source: digests["source"],
      canonical: digests["canonical"],
      "model-view": digests["model-view"],
      ...(digests["resource-manifest"] === undefined ? {} : { "resource-manifest": digests["resource-manifest"] }),
    },
  };
}

/**
 * Unwraps `core::response::Envelope<T>` (`ctx traits context plan --json`'s
 * response shape). A structurally-incompatible envelope, or an `ok: false`
 * error envelope, must throw — never silently degrade to "no traits", since
 * the resolver is the sole activation authority.
 */
function unwrapEnvelope<T>(raw: JsonValue, context: string, checkValue: (value: JsonValue, context: string) => T): T {
  if (!isRecord(raw)) {
    throw new CtxCliError(`ctx.traits: malformed envelope from ${context}: expected a JSON object`);
  }
  if (raw["ok"] !== true) {
    const error = raw["error"];
    if (isRecord(error) && typeof error["code"] === "string" && typeof error["message"] === "string") {
      throw new CtxCliError(`ctx.traits: ${context} returned an error envelope: ${error["code"]}: ${error["message"]}`);
    }
    throw new CtxCliError(`ctx.traits: ${context} returned a non-ok envelope with no structured error`);
  }
  if (raw["value"] === undefined) {
    throw new CtxCliError(`ctx.traits: ${context} returned an ok envelope with no "value"`);
  }
  return checkValue(raw["value"], context);
}

/** Every field `ContextPlanRowReport` mirrors — see {@link ContextPlanRow}. */
function isContextPlanRow(value: JsonValue): value is ContextPlanRow & JsonObject {
  return isRecord(value)
    && typeof value["trait-id"] === "string"
    && typeof value["model-view-digest"] === "string"
    && typeof value["action"] === "string"
    && (value["stale-reason"] === undefined || typeof value["stale-reason"] === "string")
    && typeof value["text"] === "string";
}

/** Full structural check for `ContextPlanReport`'s (`ContextPlanValue`) shape. */
function asContextPlanValue(raw: JsonValue, context: string): ContextPlanValue {
  if (!isRecord(raw)) {
    throw new CtxCliError(`ctx.traits: malformed response shape from ${context}: expected a JSON object`);
  }
  if (!Array.isArray(raw["rows"])) {
    throw new CtxCliError(`ctx.traits: malformed response shape from ${context}: "rows" is not an array`);
  }
  for (const row of raw["rows"]) {
    if (!isContextPlanRow(row)) {
      throw new CtxCliError(
        `ctx.traits: malformed response shape from ${context}: a "rows" entry does not match ContextPlanRow`,
      );
    }
  }
  if (
    typeof raw["host"] !== "string" || typeof raw["host-session"] !== "string"
    || typeof raw["committed"] !== "boolean" || typeof raw["note"] !== "string"
  ) {
    throw new CtxCliError(
      `ctx.traits: malformed response shape from ${context}: missing/mistyped "host"/"host-session"/"committed"/"note"`,
    );
  }
  return {
    host: raw["host"],
    "host-session": raw["host-session"],
    committed: raw["committed"],
    rows: raw["rows"] as readonly ContextPlanRow[],
    note: raw["note"],
  };
}

/**
 * The one place `HostFacts` is serialized to resolver argv: shared by every
 * verb that submits activation facts (`context plan`, `resolve`), so a new
 * predicate flag or a renamed flag only ever needs to change here.
 */
function pushActivationFactArgs(args: string[], facts: HostFacts, budgetTokens: number | undefined): void {
  for (const file of facts.files) {
    args.push("--files", file);
  }
  if (facts.mode !== undefined) {
    args.push("--mode", facts.mode);
  }
  for (const language of facts.languages) {
    args.push("--language", language);
  }
  if (budgetTokens !== undefined) {
    args.push("--budget", String(budgetTokens));
  }
}

function contextPlanArgs(
  host: string,
  hostSession: string,
  facts: HostFacts,
  budgetTokens: number | undefined,
): string[] {
  const args = [
    "traits",
    "context",
    "plan",
    "--json",
    "--host",
    host,
    "--host-session",
    hostSession,
    "--task",
    facts.task,
  ];
  pushActivationFactArgs(args, facts, budgetTokens);
  return args;
}

/**
 * REUSES `ctx traits context plan --json` (P498): resolve + render in one
 * call. Never passes `--commit` — the ledger is Family-A (claude-code/codex)
 * machinery; Family B (opencode/pi) re-asserts a sticky union every request
 * instead, so `action`/`stale-reason` are ignored by design, not oversight.
 */
export async function planContextRows(
  ctxPath: string,
  host: string,
  hostSession: string,
  facts: HostFacts,
  budgetTokens: number | undefined,
  cwd: string,
): Promise<readonly ContextPlanRow[]> {
  const context = "ctx traits context plan --json";
  const stdout = await runCtx(ctxPath, contextPlanArgs(host, hostSession, facts, budgetTokens), cwd);
  const value = unwrapEnvelope<ContextPlanValue>(parseJson(stdout, context), context, asContextPlanValue);
  return value.rows;
}

function resolveArgs(facts: HostFacts, budgetTokens: number | undefined): string[] {
  const args = ["traits", "resolve", "--json", "--task", facts.task];
  pushActivationFactArgs(args, facts, budgetTokens);
  if (facts.session !== undefined) {
    args.push("--session", facts.session);
  }
  if (facts.explicitInvocation !== undefined) {
    args.push("--explicit-invocation", facts.explicitInvocation);
  }
  if (facts.traitId !== undefined) {
    args.push("--trait-id", facts.traitId);
  }
  return args;
}

async function resolveSelectedTraitIds(
  ctxPath: string,
  facts: HostFacts,
  budgetTokens: number | undefined,
  cwd: string,
): Promise<readonly string[]> {
  const stdout = await runCtx(ctxPath, resolveArgs(facts, budgetTokens), cwd);
  const response = asResolveResponse(parseJson(stdout, "ctx traits resolve --json"), "ctx traits resolve --json");
  return (response.selected ?? []).map((candidate) => candidate["trait-id"]);
}

async function loadTraitPrompt(ctxPath: string, traitId: string, cwd: string): Promise<InjectedTrait> {
  const stdout = await runCtx(ctxPath, ["traits", "prompt", traitId, "--json"], cwd);
  const response = asPromptResponse(
    parseJson(stdout, `ctx traits prompt ${traitId} --json`),
    `ctx traits prompt ${traitId} --json`,
  );
  const modelViewDigest = response.digests?.["model-view"];
  if (typeof response.text !== "string" || typeof modelViewDigest !== "string") {
    throw new CtxCliError(`ctx.traits: \`ctx traits prompt ${traitId} --json\` returned no text/model-view digest`);
  }
  return { traitId, digest: modelViewDigest, text: response.text };
}

/** REUSES `ctx traits resolve --json` (all gating) and `ctx traits prompt --json` (compiled text + digests). Never rescores, reorders, or budgets locally. */
export async function resolveAndLoadTraits(
  ctxPath: string,
  facts: HostFacts,
  budgetTokens: number | undefined,
  cwd: string,
): Promise<readonly InjectedTrait[]> {
  const selectedIds = await resolveSelectedTraitIds(ctxPath, facts, budgetTokens, cwd);
  const injected: InjectedTrait[] = [];
  for (const traitId of selectedIds) {
    injected.push(await loadTraitPrompt(ctxPath, traitId, cwd));
  }
  return injected;
}
