# 0019 — `<information>` on both frame envelopes: budget, and prompt-over-description

**Status:** ready to implement · **Raised:** 2026-07-29

## What to add

An `<information>` element on both envelopes, carrying the context a seat needs
to do its step well — separate from the DATA it operates on.

```
<input>
  <information>
    <description>…</description>     ← only when no prompt exists
    <prompt>…</prompt>               ← wins outright
  </information>
  <data>…</data>
</input>

<output>
  <information>
    <budget>…</budget>
    <prompt>…</prompt>               ← or <description>, never both
  </information>
  <format>…</format>
  <schema>…</schema>
  <response>…</response>
</output>
```

## The precedence rule is doctrine, not a local choice

**A prompt supersedes a description outright; never emit both.** Same shape as
the config merge rule — the more specific value replaces the general one rather
than appending to it. Write it down once, in the renderer, so it is not
re-decided per call site.

`prompt.text` is not yet available as an OUTPUT field, so the first cut emits
`<description>` there. The precedence rule goes in now regardless, so adding
prompts later is additive rather than a rewrite.

## The budget: a number, not an explanation

Emit a **calculated, slightly conservative byte figure** the model can write
within. Do not explain the mechanism (that reasoning, tool traffic and the JSON
all share one stream ceiling) — that is true, and it will confuse more than it
helps.

Derive it from `COMMAND_CAPTURE_LIMIT` (327,680 bytes today) minus observed
overhead, rounded down to something memorable. The number must be honest enough
that a response inside it does not truncate.

**Why this exists:** a P535 worker frame truncated mid-response against that
ceiling and had to be re-dispatched as a fresh conversation, losing the turn.
The model had no way to know a ceiling existed. The work survived only because
the files were already written.

## Watch

- The figure must come from the SAME constant the runtime enforces. A
  hand-copied number silently drifts the day the limit changes, and the failure
  is a truncated frame — exactly what this prevents.
- `<information>` is context, `<data>` is operands. Do not let slot values leak
  into `<information>`, or the two envelopes stop meaning anything distinct.
- Keep it short. This block is paid for on every frame of every run.

## Done when

Both envelopes carry `<information>`; a prompt suppresses the description
wherever both exist; the output block states a byte budget derived from the
enforced limit; no frame explains the capture mechanism to the model.
