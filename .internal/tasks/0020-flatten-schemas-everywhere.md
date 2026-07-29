# 0020 — Flatten schemas at emit: compose in authoring, never in the payload

**Status:** ready to implement · **Raised:** 2026-07-29

## Decision

Schemas are **flattened in both consumers** — the in-prompt `<schema>` block and
the `--json-schema` payload. No `$ref`/`$defs` reaches a model or a provider.

Composition stays an AUTHORING concern: the CDK may keep a shared
`blockerSchema` referenced by several traits. The renderer inlines it.

## Why not refs

In the prompt a `$ref` forces the model to resolve indirection before it can
generate, which measurably costs reliability. For the provider payload refs
would be resolvable and therefore harmless — but keeping two shapes means two
code paths, two sets of bugs, and a standing invitation for someone to
"fix" the divergence. One flattened path is simpler and is at least as good in
both places.

## Watch

- Flattening a recursive or self-referential schema does not terminate. Reject
  such a schema at build time with a named error rather than expanding until
  something dies.
- Dedup is lost in the payload by design. If a trait's flattened schema becomes
  genuinely large, that is a signal the OUTPUT is over-specified, not that refs
  should come back.
- The `<schema>` block and the `--json-schema` payload must stay the same
  contract. Same flattening code, one source.

## Done when

No emitted schema contains `$ref` or `$defs`; shared authoring schemas still
deduplicate in source; a recursive schema fails at build with a clear message.
