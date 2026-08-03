# 0073 — The `command` channel: how anyone authors their own handoff backend

**Status:** ready to implement · **Depends on:** 0069 · **Raised:** 2026-08-03 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

Sixth slice of the handoff arc. First-party channels cover the cases we chose; this covers everyone
else's, without us shipping a plugin runtime to do it.

## Decisions

- **A subprocess is the extension point.** Envelope JSON on stdin, receipt JSON on stdout, non-zero
  exit is a failure with stderr as the recorded reason. Roughly a hundred lines to implement, no ABI
  stability promise, and it covers webhooks, PagerDuty, a Python script, an internal CLI — anything
  the machine can already run.
- **Capabilities are declared in CONFIG, not code.** A script cannot implement `capabilities()`, so
  its `[[handoff.channel]]` table states `wire`, `fidelity`, `budget-bytes` and `repeat` itself.
  First-party channels ship compiled-in defaults that a table merges over; a `command` channel has no
  defaults, so the table is required. This is the same doctrine as the harness registry, and it makes
  config the single declaration of channel shape across all kinds.
- **No templating language.** The temptation is `template = "run {{id}} is {{status}}"`, and it ends
  as a half-language with no types and no errors. The script receives the full typed `RunState` JSON
  and composes text in a real language. Config selects fidelity and wire; it does not author prose.
- **`Upsert` works here too.** The receipt's `reference` round-trips: a script that returns one gets
  it back on the next delivery and can edit whatever it created. Nothing about updating is reserved
  for first-party channels.
- **It runs with the user's privileges and we say so.** A configured command is arbitrary code from
  the repository's config, running outside any sandbox with the environment we hand it. That is the
  honest trade for zero-friction extensibility, it is stated in the docs and in `doctor` output, and
  it is the precise gap 0074 exists to close.
- **The host still owns the envelope.** A command channel receives a rendered brief and structured
  state; it does not get the ledger, the worktree or the token of any other channel. Its blast radius
  is what it can do with what it was handed.

## Scope

The `command` channel kind; the stdin/stdout envelope and receipt contract with a versioned schema;
config-declared capabilities with required fields for this kind; `resolve()` checking the program
exists and is executable; timeout and output-size bounds; `--dry-run` printing the argv, the
environment keys and the stdin payload without executing.

## Watch

- **Bound it like a command step.** An extension script that hangs must not hang the host. Reuse
  0058's liveness thinking — kill on silence with an absolute backstop — rather than inventing a
  third timeout notion.
- The receipt schema is a public contract the moment someone writes a script against it. Version it
  in the payload from the first commit; adding that later breaks every existing script.
- Do not pass secrets from other channels into the environment. A command channel gets its own
  configured env references and nothing else.
- `--dry-run` must not execute the program. A script whose dry run posts to production is exactly the
  failure `--dry-run` exists to prevent.

## Done when

A user can add a handoff backend with a config table and an executable, with no Rust and no rebuild;
the envelope and receipt schemas are versioned and documented; a script that returns a reference can
update what it created on the next delivery; a hanging or oversized script is bounded and recorded as
a failure without affecting the run; `resolve()` reports a missing or non-executable program at
`doctor` time; and `--dry-run` prints the full invocation while executing nothing.
