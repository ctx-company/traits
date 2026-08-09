/**
 * Host sink authoring (task 0110): the object-layer `sink` field and the
 * pure canonical-projection helper `effect.session.title(X)` and
 * `sink: { sessionTitle: X }` both share. Mirrors
 * `modules/core/src/trait/sink.rs`'s closed, typed `SessionTitleSink` shape
 * one-for-one; mode is inferred from the shape of `X` (owner-ratified
 * 2026-08-05), never authored separately.
 */
import type { JsonObject } from "./generated.js";
import type { PromptTemplate, SlotHandle } from "./handles.js";
import type { OptionalSlotInputValue } from "./input.js";
import { metaOf } from "./meta.js";
import { refText } from "./ref.js";

/**
 * `effect.session.title(X)`'s accepted shapes — the standard input union, no
 * new value grammar: a literal string or `input.prompt` template (verbatim),
 * or a bare slot / array of slots, optionally `input.optional(...)`-decorated
 * (generated). Composing `input.prompt` around a slot
 * (`input.prompt\`${sessionTitle}\``) is still a template — verbatim by the
 * grammar, not generated — because it matches the `PromptTemplate` arm here,
 * never the bare-`SlotHandle` arm.
 */
export type SessionTitleSinkInput =
  | string
  | PromptTemplate
  | SlotHandle
  | readonly (SlotHandle | OptionalSlotInputValue)[];

/** The object-layer `[sink.*]` declaration field on `TraitFields`/`VariantFields`. */
export interface SinkFields {
  readonly sessionTitle?: SessionTitleSinkInput;
}

function isPromptTemplate(value: unknown): value is PromptTemplate {
  return metaOf(value)?.kind === "template";
}

function isOptionalSlotInput(value: unknown): value is OptionalSlotInputValue {
  return (
    value !== null
    && typeof value === "object"
    && !Array.isArray(value)
    && "slot" in (value as Record<string, unknown>)
    && (value as { readonly optional?: unknown; }).optional === true
  );
}

/**
 * Projects an authored `effect.session.title(X)` / `sink: { sessionTitle: X
 * }` value into the canonical `[sink.session-title]` draft (`{ mode, input
 * }`) — the exact shape `modules/core/src/trait/sink.rs`'s decode validates.
 */
export function sessionTitleSinkDraft(value: SessionTitleSinkInput): JsonObject {
  if (typeof value === "string") {
    return { mode: "verbatim", input: value };
  }
  if (isPromptTemplate(value)) {
    const text = (value as unknown as { readonly text: string; }).text;
    return { mode: "verbatim", input: text };
  }
  if (Array.isArray(value)) {
    if (value.length === 0) {
      throw new Error("effect.session.title: an array input must declare at least one part");
    }
    const parts = value.map((part, index) =>
      isOptionalSlotInput(part)
        ? { slot: refText(part.slot, `effect.session.title[${index}]`), optional: true }
        : refText(part, `effect.session.title[${index}]`)
    );
    return { mode: "generated", input: parts };
  }
  return { mode: "generated", input: refText(value, "effect.session.title") };
}

const SESSION_TITLE_SINK_FIELD = "__ctxSessionTitleSink";

/**
 * Attaches an `effect.session.title(X)` declaration collected inside
 * `procedure.from(...)`'s body onto the returned `ProcedureHandle`, as a
 * hidden (non-enumerable) field — the same `withHiddenField` idiom
 * `sequence.check`'s `pass` uses — so `canonicalTraitFields` can read it back
 * off `fields.procedure` without a new `TraitFields` position, and it never
 * serializes into canonical JSON by accident.
 */
export function attachSessionTitleSink<T extends object>(handle: T, value: SessionTitleSinkInput): T {
  Object.defineProperty(handle, SESSION_TITLE_SINK_FIELD, { value, enumerable: false, configurable: true });
  return handle;
}

/** Reads an `effect.session.title(X)` declaration attached by {@link attachSessionTitleSink}, or `undefined`. */
export function readSessionTitleSink(handle: unknown): SessionTitleSinkInput | undefined {
  if (handle === null || typeof handle !== "object") return undefined;
  const field = (handle as Record<string, unknown>)[SESSION_TITLE_SINK_FIELD];
  return field as SessionTitleSinkInput | undefined;
}
