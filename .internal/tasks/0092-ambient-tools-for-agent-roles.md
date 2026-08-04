# 0092 — Ambient tools for agent roles, and why they are not peers

**Status:** filed, not scheduled (owner call 2026-08-04 — real and wanted, but its determinism story must be settled before it ships) · **Depends on:** 0090 (shared server declarations) · **Raised:** 2026-08-04 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

A worker seat with context7 available throughout its turn, calling it whenever it judges useful. This
is genuinely wanted and it is genuinely not the peer interface, and the difference is worth writing
down before someone implements one as the other.

## The distinction

A `consult` (0087) is **structural**: the trait author decides that at *this* point a question is
asked, it is one entry in the ledger, its request is inspectable before its effect fires, and it
appears in the run's graph. An ambient tool call is **model-driven**: it happens inside a turn,
possibly many times, chosen by the model, and what survives is a trajectory in a transcript rather
than a step in a procedure.

Same underlying servers. Completely different contract with the ledger.

## Decisions

- **It is harness configuration, not procedure structure.** Grants live with the seat —
  `[agent.role.*]`, beside the model and the harness — because "which tools does this seat hold" is a
  property of the agent, not of the step it happens to be running.
- **Server declarations are shared with 0090**, which is the one thing that must be true from the
  start. Two configs describing the same stdio command with different environments is a bug waiting
  for a Friday, and it is why 0090 declares servers separately from peers rather than inline.
- **Grants are explicit and per-role.** A named list of tools per seat. No ambient-everything, no
  inheritance from a global set — a seat's reach should be readable in one table.
- **The host still performs every call.** Whatever the model decides, the effect is executed outside
  the sandbox by the host with its own credentials, and the request record exists before it fires
  (0087). If an ambient tool call bypasses that, the isolation argument and the mediation point
  collapse together, and they are the two properties the whole design was built to keep.
- **A tool that writes is a second author.** 0009 refuses this for the guide seat for the right
  reason: a procedure whose claim is that every write is typed and attributed cannot have an unlogged
  writer inside a model turn. Read-shaped tools first; a writing grant needs its own argument.

## The open question that blocks it

**What does replay mean for a turn with tool calls in it?** A recorded run replays recorded values;
a turn that made four tool calls has four results that are part of how the response came to be but are
not accepted slot values. Either they become ledger-visible — which is a lot of machinery and makes
every turn a subgraph — or a replayed turn replays its transcript, tool results included, which means
the transcript becomes evidence rather than a record. Both are defensible. Neither is decided, and
shipping without deciding produces a run that cannot honestly claim to be reproducible.

## Watch

- **Cost.** A seat with tools uses them, repeatedly, and the calls are invisible until the bill.
  Per-role accounting has to cover them the way 0009 requires it for the guide.
- Latency inside a turn is unbounded in a way a step's is not. 0084's budget is per step; a turn making
  six calls needs its own answer.
- A grant is a capability, and the model chooses when to exercise it. That is a materially weaker
  guarantee than a declared consult, and any surface that reports what a run reached out to must
  distinguish the two rather than listing them together.
- Do not let this arrive as "peers, but the model picks". It shares transport and nothing else.

## Done when

An agent role can be granted named tools from a shared server declaration; the host performs every
call with the request recorded before its effect; grants are readable per seat; tool use lands in the
run's accounting; replay behaviour for a turn containing tool calls is decided and documented rather
than incidental; and a run's report distinguishes model-chosen tool use from declared consults.
