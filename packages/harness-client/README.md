# `@ctx-traits/harness-client`

The shared `ctx` CLI wire client for every Family-B host adapter (OpenCode,
Pi): spawning `ctx`, parsing and structurally validating its JSON/envelope
responses, building activation-fact argv, and the sticky-union provenance
block (`UnionEntry`/`provenanceBlock`) each adapter re-asserts every turn.
One implementation of the wire contract, not one per host — a flag rename
on the CLI side only ever needs one edit here.

No resolver logic lives here: this package is IO + parsing glue only.
`ctx traits resolve`/`context plan`/`prompt` remain the sole activation
authority; a structurally-unexpected response throws (`CtxCliError`)
rather than silently degrading to "no traits".

## Publishing

Both consumers (`.configs/plugins/opencode`, `.configs/plugins/pi`) depend
on this package via `workspace:*`, which pnpm rewrites to a real version
range on publish. This package is not useful standalone, and pnpm's publish
step requires the workspace dependency to already have a matching published
version available — so this package must be published in the same release
step as (or before) either plugin, never on its own separate cadence.
