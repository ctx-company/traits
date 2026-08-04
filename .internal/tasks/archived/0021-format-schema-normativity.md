# 0021 — `<response>` must point at `<schema>`, not `<format>`

**Status:** ready to implement · **Raised:** 2026-07-29

## The bug

A frame currently emits both:

```
<format>
  { "review-verdict": review-verdict }
</format>

<schema>
  <review-verdict>{"additionalProperties":false,"properties":{…}}</review-verdict>
</schema>

<response>
  Return ONLY one JSON object matching <format> …
</response>
```

`<format>` is a sketch whose value is a bare unquoted token — not valid JSON,
not a type, not an example. `<schema>` is the real contract, with types and
per-field descriptions.

**`<response>` sends the model to the weaker artifact.** The instruction and the
contract disagree about which is normative.

## What to do

Make `<schema>` normative in the instruction. Keep `<format>` only if it earns
its place as a shape-at-a-glance summary — and if kept, make it valid JSON with
type placeholders rather than bare tokens, so it cannot be read as an example
that conflicts with the schema.

## Watch

- Do not simply delete `<format>`: a one-line shape is genuinely useful for a
  large schema. The defect is that it is unparseable AND cited as authoritative.
- Whatever the wording becomes, it is paid on every frame — keep it one line.

## Done when

`<response>` names the authoritative artifact; any surviving `<format>` is valid
JSON; no frame instructs the model to match a non-normative sketch.
