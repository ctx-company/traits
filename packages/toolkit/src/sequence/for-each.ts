import type {
  ForEachSequenceFields,
  JsonValue,
  RefHandle,
  SequenceHandle,
  SequenceOutputValue,
  SlotHandle,
} from "@ctx-traits/cdk";
import { sequence } from "@ctx-traits/cdk";

/** `sequence.forEach`'s own `sequence` field type — not itself exported by `@ctx-traits/cdk`, so derived from the field it governs. */
type SequenceRef = NonNullable<ForEachSequenceFields["sequence"]>;

export type ForEachJoinedOptions<Item = JsonValue> = {
  readonly id: string;
  readonly title?: string;
  /** The list slot to fan out over. */
  readonly over: string | SlotHandle<readonly Item[]> | RefHandle;
  /** The per-item slot each round binds the current element to. */
  readonly item: string | SlotHandle<Item> | RefHandle;
  /** The per-item body: an inline step list (wrapped in a `sequence.linear` named `<id>-body`, matching `sequence.forEach`'s own auto-naming) or a caller-owned named-sequence ref. */
  readonly body: readonly SequenceHandle[] | SequenceRef;
  /** The slot each round's body appends its result onto — the joined output every item's work accumulates into. */
  readonly output: SequenceOutputValue | readonly SequenceOutputValue[];
  /** Caps the number of elements processed; extras are skipped deterministically. */
  readonly maxItems?: number;
  readonly limit?: number;
  readonly concurrent?: boolean;
};

/**
 * Per-item processing with a joined result: runs `body` once per element of
 * `over`, binding each element to `item`, and accumulates every round's
 * result into `output`. A thin, named wrapper over `sequence.forEach`
 * (`packages/cdk/src/sequence.ts`) — every field maps straight through, so
 * the emitted canonical is identical to calling `sequence.forEach` by hand
 * with the same fields. Exists so a per-item-join fan-out reads as one
 * named call at its use site rather than requiring the caller to know the
 * CDK's own `over`/`item`/`sequence` step-kind vocabulary.
 * @example
 * ```ts
 * forEachJoined({
 *   id: "research-streams",
 *   over: streamList,
 *   item: currentStream,
 *   body: [researchOneStream],
 *   output: findings,
 *   maxItems: 6,
 * });
 * ```
 */
export function forEachJoined<Item = JsonValue>(options: ForEachJoinedOptions<Item>): SequenceHandle {
  const { id, title, over, item, body, output, maxItems, limit, concurrent } = options;
  // `Array.isArray` does not negatively narrow `body` here (its non-array
  // arm, `SequenceRef`, is itself a plain-object handle type TS cannot
  // statically prove disjoint from `readonly SequenceHandle[]`) — the cast
  // below is a runtime-true, type-only assertion, not a behavior change.
  const bodyRef: SequenceRef = Array.isArray(body)
    ? sequence.linear(`${id}-body`, body)
    : (body as SequenceRef);
  return sequence.forEach(id, {
    ...(title === undefined ? {} : { title }),
    over,
    item,
    sequence: bodyRef,
    output,
    ...(maxItems === undefined ? {} : { maxItems }),
    ...(limit === undefined ? {} : { limit }),
    ...(concurrent === undefined ? {} : { concurrent }),
  });
}
