# 0025 — Declare a role once as an array instead of `smart-1`/`smart-2`

**Status:** ready to implement · **Raised:** 2026-07-29

Today two reviewer seats mean two near-identical config blocks,
`[agent.role.smart-1]` and `[agent.role.smart-2]`, differing in nothing but the
suffix. Allow one declaration covering N seats:

```toml
[agent.role.smart]
harness = "opencode"
model = "openai/gpt-5.6-sol"
count = 2          # or an array form if seats must differ
```

## Watch

- **Seat identity is load-bearing.** Traits address `agent:smart-1` and
  `agent:smart-2` by name, park reports attribute blockers per seat, and the
  dual-review contract turns on the two being distinguishable. Expansion must
  produce stable, addressable ids — not anonymous replicas.
- Seats that must genuinely DIFFER (different models, deliberately) still need
  per-seat expression. An array of tables covers both: one entry means one
  seat, N entries mean N, and each entry may override.
- The resolved view (`doctor --config`) should keep showing the expanded seats,
  since that is what actually runs. Collapse the authoring, not the report.
- Trait-declared agent ids stay the contract. This is config-side sugar and
  must not require a trait to know how many seats a role expands to.

## Done when

One `[agent.role.<name>]` declaration yields N addressable seats with stable
ids; a per-seat override is still expressible; `doctor --config` shows the
expanded seats; existing `smart-1`/`smart-2` declarations keep working.
