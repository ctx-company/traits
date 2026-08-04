# 0090 — The `mcp` peer: server, toolset, binding — with the tool's schema checked at `doctor`

**Status:** ready to implement, but ranked last in the arc (see below) · **Depends on:** 0087 · **Raised:** 2026-08-04 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

Fourth slice of the peer arc.

**This is not the research interface, and it should not be sold as one.** A worker looking things up
while it codes is better served by an ambient tool grant (0092): the model knows when it needs
something and an author cannot predict it. What this is for is narrower and real — **a tool result as
typed state**: ask the tracker for acceptance criteria the reviewer step then checks against, ask a
release calendar and branch on the answer, ask an index for a value the procedure's control flow
depends on. 0087's four cases, applied to a machine that answers instead of a person.

Ranked last because 0088 is the slice with no substitute, while this one's use cases are a narrow
subset of what 0092 covers less rigorously but far more usefully.

## Decisions

- **Three levels, declared separately: server, toolset, binding.**
  - `[[mcp.server]]` — connection, transport, credentials, environment. Declared once.
  - `[[mcp.toolset]]` — a named slice of reach over one server: which tools, arguments pinned, bounds.
  - `[[peer]]` — one callable drawn from a toolset, which an `ask` binds to.
- **The toolset is the unit of granted reach**, and that is its point rather than convenience. A
  policy layer names toolsets instead of enumerating tools, and 0092 grants the same toolset to an
  agent seat. One declaration, two consumers: a declared `ask` and an ambient grant.
- **A peer is still one callable.** That is what keeps the `doctor` check possible, and adding a
  second tool is one more `[[peer]]` line over the same toolset — no new server, no new credentials.
- **`doctor` checks both schemas.** It reads the tool's published input and output schema and matches
  them against what the trait's `ask` declares. A missing tool, an unreachable server, a renamed
  argument or a reply shape that no longer fits fails there. This is the strongest static check in the
  arc and it is nearly free, because MCP tools publish JSON Schema.
- **`args` is a field rename map, not a template.** `args = { query = "question" }` maps request
  fields to tool arguments by name; unmapped fields match by name. 0073's rule holds — no
  half-language with no types and no errors.
- **Sequential tools stay sequential steps.** A server needing resolve-then-fetch is two peers and two
  asks, both visible in the procedure. Chaining inside one peer would hide a call, which is the one
  thing a declared ask exists to prevent.
- **The host owns the server process**, as it owns every other effect: started, bounded and torn down
  outside the sandbox with the host's environment. A run never launches an MCP server.
- **Answers by default, tells off.** A side-effecting tool is still modelled as an ask that returns a
  result; there is no useful `tell` against a tool that reports what it did.

## Scope

The `mcp` peer variant; `[[mcp.server]]` and `[[mcp.toolset]]` tables with peers bound over them;
stdio and HTTP transports as the server config allows; tool discovery and both-direction schema
matching in `resolve()`; the `args` rename map; host-owned server lifecycle with bounds from 0084;
`--dry-run` printing the resolved server, tool and arguments without calling.

## Watch

- **Tool output is untrusted input** and is validated against the declared reply schema before it
  becomes a slot value (0088). A published schema is a claim, not a guarantee.
- **Schema drift is a live hazard, not just a `doctor` one.** A server upgraded between `doctor` and a
  run changes the contract underneath it. The reply check catches it, but the message must name the
  server and tool or it reads as a trait bug.
- A slow or dead server parks the run as a fault, not as an open question (`deferred` stays false).
  Getting that default wrong makes an outage look like it is waiting for a human.
- Third-party servers carry their own auth and reach their own network. Credentials are env references
  resolved in the host and never printed (0069); reachability is a `doctor` line.
- Do not let a toolset become ambient access by being bound to a peer. A peer is called at a declared
  point; a grant lets a model call at will, and that is 0092 with a different determinism story.

## Done when

Servers, toolsets and peers are declared at three levels with a toolset reusable by 0092; a `[[peer]]`
binds one tool from a toolset; `doctor` matches the tool's published input and output schemas against
the trait's declared request and reply and fails on a mismatch, a missing tool or an unreachable
server; an ask calls it and lands a validated typed reply in the ledger; the host owns the process
lifecycle; and `--dry-run` prints the resolved call without making it.
